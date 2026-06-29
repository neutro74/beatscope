//! Spectrum analysis: windowed FFT mapped to log-spaced frequency bands, with
//! cava-style smoothing (fast rise, gravity-driven fall, neighbour spread).

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::audio::{FFT_SIZE, SAMPLE_RATE};
use crate::config::Config;

pub struct Analyzer {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    scratch: Vec<Complex<f32>>,
    mag: Vec<f32>,

    bars: usize,
    band_lo: Vec<usize>,
    band_hi: Vec<usize>,
    weight: Vec<f32>,

    smoothed: Vec<f32>,
    vel: Vec<f32>,

    // tunables (mirrored from Config, live-editable)
    pub gain: f32,
    pub auto_gain: bool,
    pub low_hz: f32,
    pub high_hz: f32,
    pub noise_floor_db: f32,
    pub ceiling_db: f32,
    pub rise: f32,
    pub gravity: f32,
    pub smoothing: f32,

    agc: f32,
}

impl Analyzer {
    pub fn new(cfg: &Config) -> Analyzer {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);

        // Hann window.
        let window = (0..FFT_SIZE)
            .map(|n| {
                let x = std::f32::consts::PI * n as f32 / (FFT_SIZE as f32 - 1.0);
                x.sin().powi(2)
            })
            .collect();

        let mut a = Analyzer {
            fft,
            window,
            scratch: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            mag: vec![0.0; FFT_SIZE / 2],
            bars: 0,
            band_lo: Vec::new(),
            band_hi: Vec::new(),
            weight: Vec::new(),
            smoothed: Vec::new(),
            vel: Vec::new(),
            gain: cfg.gain,
            auto_gain: cfg.auto_gain,
            low_hz: cfg.low_hz,
            high_hz: cfg.high_hz,
            noise_floor_db: cfg.noise_floor_db,
            ceiling_db: cfg.ceiling_db,
            rise: cfg.rise,
            gravity: cfg.gravity,
            smoothing: cfg.smoothing,
            agc: 1.0,
        };
        a.set_bars(64);
        a
    }

    /// (Re)configure the number of output bands and their frequency boundaries.
    pub fn set_bars(&mut self, n: usize) {
        let n = n.max(1);
        if n == self.bars && !self.band_lo.is_empty() {
            return;
        }
        self.bars = n;
        self.band_lo = vec![0; n];
        self.band_hi = vec![0; n];
        self.weight = vec![1.0; n];
        self.smoothed = vec![0.0; n];
        self.vel = vec![0.0; n];
        self.rebuild_bands();
    }

    pub fn rebuild_bands(&mut self) {
        let n = self.bars;
        let nyquist = SAMPLE_RATE as f32 / 2.0;
        let lo = self.low_hz.clamp(1.0, nyquist - 1.0);
        let hi = self.high_hz.clamp(lo + 1.0, nyquist);
        let bin_hz = SAMPLE_RATE as f32 / FFT_SIZE as f32;
        let log_lo = lo.ln();
        let log_hi = hi.ln();

        for i in 0..n {
            let f0 = (log_lo + (log_hi - log_lo) * i as f32 / n as f32).exp();
            let f1 = (log_lo + (log_hi - log_lo) * (i + 1) as f32 / n as f32).exp();
            let mut b0 = (f0 / bin_hz).floor() as usize;
            let mut b1 = (f1 / bin_hz).ceil() as usize;
            b0 = b0.clamp(1, FFT_SIZE / 2 - 1);
            b1 = b1.clamp(b0 + 1, FFT_SIZE / 2);
            self.band_lo[i] = b0;
            self.band_hi[i] = b1;
            // Gently lift higher bands which carry less energy.
            let center = (f0 * f1).sqrt();
            self.weight[i] = 1.0 + 0.8 * (center / hi);
        }
    }

    /// Process the latest samples and return per-band levels in 0..=1.
    pub fn compute(&mut self, samples: &[f32], dt: f32) -> &[f32] {
        let n = samples.len().min(FFT_SIZE);
        let off = samples.len() - n;
        for i in 0..FFT_SIZE {
            let s = if i < n { samples[off + i] } else { 0.0 };
            self.scratch[i] = Complex::new(s * self.window[i], 0.0);
        }
        self.fft.process(&mut self.scratch);

        let norm = 2.0 / FFT_SIZE as f32;
        for k in 0..FFT_SIZE / 2 {
            self.mag[k] = self.scratch[k].norm() * norm;
        }

        // Map magnitude (dB) into 0..1 across a floor..ceiling window. This gives
        // a fixed, predictable dynamic range so loud passages don't instantly clip.
        let floor = self.noise_floor_db;
        let span = (self.ceiling_db - floor).max(6.0);
        let mut frame_peak = 1e-6f32;

        for i in 0..self.bars {
            let (b0, b1) = (self.band_lo[i], self.band_hi[i]);
            let mut sum = 0.0;
            for k in b0..b1 {
                sum += self.mag[k];
            }
            let mean = (sum / (b1 - b0) as f32) * self.weight[i];
            let db = 20.0 * (mean + 1e-9).log10();
            let raw = ((db - floor) / span) * self.gain;
            frame_peak = frame_peak.max(raw);
            let t = (raw * self.agc).clamp(0.0, 1.0);

            // Time response: snap up, gravity down.
            if t >= self.smoothed[i] {
                self.smoothed[i] += (t - self.smoothed[i]) * self.rise;
                self.vel[i] = 0.0;
            } else {
                self.vel[i] += self.gravity * dt;
                self.smoothed[i] = (self.smoothed[i] - self.vel[i] * dt).max(t).max(0.0);
            }
        }

        // Auto-gain: nudge a single scalar so the loudest band tends toward ~0.7,
        // leaving headroom so peaks don't slam the top of the panel. Stay within a
        // limited range and adapt slowly so quiet stays quiet.
        if self.auto_gain {
            if frame_peak > 0.02 {
                let desired = (0.70 / frame_peak).clamp(0.4, 3.0);
                let rate = if desired < self.agc { 0.03 } else { 0.12 };
                self.agc += (desired - self.agc) * rate;
            }
        } else {
            self.agc = 1.0;
        }

        // Neighbour spread (monstercat-style) for a smoother silhouette.
        if self.smoothing > 0.0 && self.bars > 2 {
            let spread = self.smoothing;
            let src = self.smoothed.clone();
            for i in 0..self.bars {
                let mut v = src[i];
                if i > 0 {
                    v = v.max(src[i - 1] * spread);
                }
                if i + 1 < self.bars {
                    v = v.max(src[i + 1] * spread);
                }
                self.smoothed[i] = v;
            }
        }

        &self.smoothed
    }
}
