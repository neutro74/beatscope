//! Configuration: persisted TOML settings plus command-line overrides.

use std::path::PathBuf;

use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::theme::Palette;
use crate::visualizer::VisMode;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// PulseAudio/PipeWire source to capture. None => autodetect default sink monitor.
    pub source: Option<String>,
    /// Target frames per second.
    pub fps: u32,
    /// Visualizer rendering mode.
    pub mode: VisMode,
    /// Color palette.
    pub palette: Palette,
    /// Side the song panel sits on ("left" or "right").
    pub song_side: Side,

    // --- lyrics ---
    /// Lyrics panel mode: off, full (replaces visualizer), or split (both).
    pub lyrics_mode: LyricsMode,
    /// Show pronunciation (romanization) under each line.
    pub lyrics_romaji: bool,
    /// Fetch and show English translation under each line.
    pub lyrics_translate: bool,
    /// Manual sync trim (seconds), applied on top of the auto-measured latency.
    pub lyrics_offset: f64,
    /// Auto-measured audio output latency (seconds): how far the player's
    /// reported position leads the sound you actually hear. Learned at runtime
    /// from play/resume transients and persisted so it's known next launch.
    pub lyrics_latency: f64,

    // --- visualizer feel ---
    /// Bar width in cells (bar modes).
    pub bar_width: u16,
    /// Gap between bars in cells.
    pub bar_gap: u16,
    /// Lowest frequency mapped (Hz).
    pub low_hz: f32,
    /// Highest frequency mapped (Hz).
    pub high_hz: f32,
    /// Manual sensitivity multiplier.
    pub gain: f32,
    /// Adapt gain to recent peaks.
    pub auto_gain: bool,
    /// Noise floor in dB (bottom of the bar range; more negative = more sensitive).
    pub noise_floor_db: f32,
    /// Ceiling in dB (top of the bar range; bands at/above this fill the panel).
    pub ceiling_db: f32,
    /// Rise smoothing 0..1 (higher = snappier).
    pub rise: f32,
    /// Gravity for falling bars.
    pub gravity: f32,
    /// Neighbour smoothing 0..1 (monstercat-style spread).
    pub smoothing: f32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LyricsMode {
    Off,
    Full,
    Split,
}

impl LyricsMode {
    pub fn next(self) -> LyricsMode {
        match self {
            LyricsMode::Off => LyricsMode::Full,
            LyricsMode::Full => LyricsMode::Split,
            LyricsMode::Split => LyricsMode::Off,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LyricsMode::Off => "off",
            LyricsMode::Full => "full",
            LyricsMode::Split => "split",
        }
    }

    pub fn active(self) -> bool {
        !matches!(self, LyricsMode::Off)
    }
}

impl Side {
    pub fn flip(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            source: None,
            fps: 60,
            mode: VisMode::Bars,
            palette: Palette::Spectrum,
            song_side: Side::Left,
            lyrics_mode: LyricsMode::Off,
            lyrics_romaji: true,
            lyrics_translate: false,
            lyrics_offset: 0.0,
            lyrics_latency: 0.0,
            bar_width: 2,
            bar_gap: 1,
            low_hz: 30.0,
            high_hz: 16_000.0,
            gain: 1.0,
            auto_gain: true,
            noise_floor_db: -58.0,
            ceiling_db: -12.0,
            rise: 0.45,
            gravity: 7.0,
            smoothing: 0.35,
        }
    }
}

impl Config {
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("beatscope").join("config.toml"))
    }

    /// Load from disk, falling back to defaults on any error.
    pub fn load() -> Config {
        let Some(path) = Self::config_path() else {
            return Config::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                eprintln!("beatscope: invalid config ({e}); using defaults");
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    /// Persist current settings to disk. Errors are returned but non-fatal.
    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = Self::config_path() else {
            anyhow::bail!("no config dir");
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Apply non-None CLI overrides on top of loaded config.
    pub fn apply_cli(&mut self, cli: &Cli) {
        if let Some(s) = &cli.source {
            self.source = Some(s.clone());
        }
        if let Some(f) = cli.fps {
            self.fps = f.clamp(5, 240);
        }
        if let Some(m) = cli.mode {
            self.mode = m;
        }
        if let Some(p) = cli.palette {
            self.palette = p;
        }
        if let Some(g) = cli.gain {
            self.gain = g;
        }
    }
}

/// A better terminal music visualizer: album art + controls beside a configurable spectrum.
#[derive(Parser, Debug)]
#[command(name = "beatscope", version, about)]
pub struct Cli {
    /// Capture source (PulseAudio/PipeWire). Default: monitor of the default sink.
    #[arg(short, long)]
    pub source: Option<String>,
    /// Target frames per second.
    #[arg(long)]
    pub fps: Option<u32>,
    /// Visualizer mode: bars, mirror, wave, circle, spectrum.
    #[arg(short, long)]
    pub mode: Option<VisMode>,
    /// Color palette: spectrum, fire, ocean, aurora, rainbow, mono.
    #[arg(short, long)]
    pub palette: Option<Palette>,
    /// Sensitivity multiplier.
    #[arg(short, long)]
    pub gain: Option<f32>,
    /// List available capture sources and exit.
    #[arg(long)]
    pub list_sources: bool,
    /// Update beatscope to the latest GitHub release and exit.
    #[arg(long)]
    pub update: bool,
    /// With --update, reinstall the latest release even if already current.
    #[arg(long)]
    pub force: bool,
}
