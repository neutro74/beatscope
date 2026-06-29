//! PulseAudio/PipeWire capture running on a background thread.
//!
//! We open a record stream against a monitor source (the audio currently being
//! played) and keep the most recent `FFT_SIZE` mono samples in a shared ring.

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use libpulse_binding::def::BufferAttr;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;
use libpulse_simple_binding::Simple;

pub const SAMPLE_RATE: u32 = 44_100;
pub const FFT_SIZE: usize = 4096;

/// Shared handle to the latest captured audio window.
#[derive(Clone)]
pub struct AudioCapture {
    buf: Arc<Mutex<Vec<f32>>>,
    connected: Arc<Mutex<bool>>,
}

impl AudioCapture {
    pub fn start(source: Option<String>) -> AudioCapture {
        let buf = Arc::new(Mutex::new(vec![0.0f32; FFT_SIZE]));
        let connected = Arc::new(Mutex::new(false));
        let cap = AudioCapture {
            buf: buf.clone(),
            connected: connected.clone(),
        };

        let dev = source.or_else(default_monitor_source);
        thread::spawn(move || loop {
            if let Err(_e) = capture_loop(&buf, &connected, dev.as_deref()) {
                *connected.lock().unwrap() = false;
                thread::sleep(Duration::from_millis(750));
            }
        });
        cap
    }

    /// Copy the latest window of samples into `out`.
    pub fn snapshot(&self, out: &mut Vec<f32>) {
        let g = self.buf.lock().unwrap();
        out.clear();
        out.extend_from_slice(&g);
    }

    pub fn is_connected(&self) -> bool {
        *self.connected.lock().unwrap()
    }
}

fn capture_loop(
    buf: &Arc<Mutex<Vec<f32>>>,
    connected: &Arc<Mutex<bool>>,
    dev: Option<&str>,
) -> anyhow::Result<()> {
    let spec = Spec {
        format: Format::F32le,
        channels: 1,
        rate: SAMPLE_RATE,
    };
    if !spec.is_valid() {
        anyhow::bail!("invalid sample spec");
    }

    // Request a small fragment so the monitor stream stays low-latency; without
    // this the server picks a large buffer and the visualizer lags the audio.
    const CHUNK_FRAMES: usize = 256;
    let frag = (CHUNK_FRAMES * 4) as u32;
    let attr = BufferAttr {
        maxlength: frag * 4,
        tlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: frag,
    };

    let simple = Simple::new(
        None,
        "beatscope",
        Direction::Record,
        dev,
        "visualizer",
        &spec,
        None,
        Some(&attr),
    )
    .map_err(|e| anyhow::anyhow!("pulse connect failed: {e}"))?;

    *connected.lock().unwrap() = true;

    let mut raw = vec![0u8; CHUNK_FRAMES * 4];
    loop {
        simple
            .read(&mut raw)
            .map_err(|e| anyhow::anyhow!("pulse read failed: {e}"))?;

        let mut g = buf.lock().unwrap();
        for c in raw.chunks_exact(4) {
            g.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
        }
        let len = g.len();
        if len > FFT_SIZE {
            g.drain(0..len - FFT_SIZE);
        }
    }
}

/// Best-effort detection of the default sink's monitor source via `pactl`.
pub fn default_monitor_source() -> Option<String> {
    let out = Command::new("pactl").arg("get-default-sink").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let sink = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sink.is_empty() {
        return None;
    }
    Some(format!("{sink}.monitor"))
}

/// List capture sources (for `--list-sources`).
pub fn list_sources() -> Vec<String> {
    let Ok(out) = Command::new("pactl")
        .args(["list", "short", "sources"])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split('\t').nth(1).map(str::to_string))
        .collect()
}
