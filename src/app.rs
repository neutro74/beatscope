//! Application state, the update/draw cycle, and input handling.

use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;

use crate::art::ArtLoader;
use crate::audio::AudioCapture;
use crate::config::{Config, LyricsMode, Side};
use crate::dsp::Analyzer;
use crate::lyrics::{Lyrics, LyricsManager, Request};
use crate::lyrics_ui::LyricsView;
use crate::player::{Command, NowPlaying, PlayerHandle};
use crate::ui;
use crate::visualizer::{self, VisMode, VizParams};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    Help,
    Modes,
    Palettes,
}

/// Latency auto-calibration state. We arm at a known song-position (a resume from
/// pause, or a track start) and, when sound first appears in the captured audio,
/// the gap between the current player position and that start position is the
/// real output latency.
#[derive(Clone, Copy)]
enum Cal {
    Idle,
    Armed { start_pos: f64, since: Instant },
}

pub struct App {
    cfg: Config,
    audio: AudioCapture,
    analyzer: Analyzer,
    player: PlayerHandle,
    art: ArtLoader,
    picker: Picker,
    protocol: Option<StatefulProtocol>,
    art_src: Option<image::DynamicImage>,
    proto_dims: (u16, u16),
    last_art_url: Option<String>,

    samples: Vec<f32>,
    np: NowPlaying,
    last_frame: Instant,
    toast: Option<(String, Instant)>,
    overlay: Overlay,
    picker_cursor: usize,
    picker_saved: (VisMode, crate::theme::Palette),
    should_quit: bool,

    hits: ui::SongHits,
    dragging: bool,
    drag_frac: Option<f64>,
    last_seek: Instant,
    wave: Vec<f32>,
    peaks: Vec<f32>,
    /// Bass-energy envelope (fast attack, slow decay) driving the circle pulse.
    beat: f32,
    /// Rolling spectrum history for the scrolling spectrogram mode.
    spectro: std::collections::VecDeque<Vec<f32>>,

    lyrics: LyricsManager,
    lyrics_snapshot: Lyrics,
    last_lyrics_key: String,
    fetched_translate: bool,
    fetched_pronounce: bool,
    start: Instant,

    // Latency auto-calibration.
    latency: f64,
    cal: Cal,
    paused_pos: f64,
    was_playing: bool,
    audio_loud: bool,
    cal_prev_title: String,
}

impl App {
    pub fn new(cfg: Config) -> Result<App> {
        // Query the terminal for graphics support *before* entering raw mode.
        let mut picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
        // The user specifically wants the Kitty graphics protocol.
        picker.set_protocol_type(ProtocolType::Kitty);

        let analyzer = Analyzer::new(&cfg);
        let audio = AudioCapture::start(cfg.source.clone());
        let player = PlayerHandle::start();
        let art = ArtLoader::start();
        let lyrics = LyricsManager::start();
        let cfg_latency = cfg.lyrics_latency;

        Ok(App {
            cfg,
            audio,
            analyzer,
            player,
            art,
            picker,
            protocol: None,
            art_src: None,
            proto_dims: (0, 0),
            last_art_url: None,
            samples: Vec::new(),
            np: NowPlaying::default(),
            last_frame: Instant::now(),
            toast: None,
            overlay: Overlay::None,
            picker_cursor: 0,
            picker_saved: (VisMode::Bars, crate::theme::Palette::Spectrum),
            should_quit: false,
            hits: ui::SongHits::default(),
            dragging: false,
            drag_frac: None,
            last_seek: Instant::now(),
            wave: Vec::new(),
            peaks: Vec::new(),
            beat: 0.0,
            spectro: std::collections::VecDeque::new(),
            lyrics,
            lyrics_snapshot: Lyrics::default(),
            last_lyrics_key: String::new(),
            fetched_translate: false,
            fetched_pronounce: false,
            start: Instant::now(),
            latency: cfg_latency,
            cal: Cal::Idle,
            paused_pos: 0.0,
            was_playing: false,
            audio_loud: false,
            cal_prev_title: String::new(),
        })
    }

    fn request_lyrics(&mut self) {
        let key = format!("{}|{}", self.np.artist, self.np.title);
        self.last_lyrics_key = key.clone();
        // Remember what this fetch will include, so toggles don't refetch in vain.
        self.fetched_translate = self.cfg.lyrics_translate;
        self.fetched_pronounce = self.cfg.lyrics_romaji;
        self.lyrics.request(Request {
            key,
            artist: self.np.artist.clone(),
            title: self.np.title.clone(),
            album: self.np.album.clone(),
            duration: self.np.length,
            translate: self.cfg.lyrics_translate,
            pronounce: self.cfg.lyrics_romaji,
        });
    }

    /// Fetch only if a now-enabled extra (translation/pronunciation) hasn't been
    /// retrieved for the current track yet.
    fn ensure_lyrics_extras(&mut self) {
        if !self.cfg.lyrics_mode.active() || self.np.title.is_empty() {
            return;
        }
        let need = (self.cfg.lyrics_translate && !self.fetched_translate)
            || (self.cfg.lyrics_romaji && !self.fetched_pronounce);
        if need {
            self.request_lyrics();
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let frame_dur = Duration::from_secs_f64(1.0 / self.cfg.fps.max(5) as f64);
        while !self.should_quit {
            let frame_start = Instant::now();
            self.update();
            terminal.draw(|f| self.draw(f))?;

            let elapsed = frame_start.elapsed();
            let timeout = frame_dur.saturating_sub(elapsed);
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) => self.on_key(key),
                    Event::Mouse(me) => self.on_mouse(me),
                    _ => {}
                }
            }
        }
        // Persist current settings (palette, mode, gain, …) so they survive exit.
        let _ = self.cfg.save();
        Ok(())
    }

    fn update(&mut self) {
        self.np = self.player.snapshot();

        // Album art changed? Request the new image but keep the current one on
        // screen until the replacement actually decodes. Blanking immediately
        // meant a replay or quick track change (where the player rewrites/rotates
        // its art cache file) showed a broken/empty panel while the reload — and
        // its retries — ran. Only clear when the track genuinely has no art.
        if self.np.art_url != self.last_art_url {
            self.last_art_url = self.np.art_url.clone();
            match &self.np.art_url {
                Some(url) => self.art.request(url.clone()),
                None => {
                    self.art_src = None;
                    self.protocol = None;
                    self.proto_dims = (0, 0);
                }
            }
        }

        // Track changed? Refresh lyrics (only while the lyrics panel is in use).
        let key = format!("{}|{}", self.np.artist, self.np.title);
        if self.cfg.lyrics_mode.active() && key != self.last_lyrics_key && !self.np.title.is_empty()
        {
            self.request_lyrics();
        }
        self.lyrics_snapshot = self.lyrics.snapshot();
        // New decoded source: stash it and force the protocol to rebuild (the
        // actual high-quality resize happens in draw, where the area is known).
        if let Some(img) = self.art.poll() {
            self.art_src = Some(img);
            self.protocol = None;
            self.proto_dims = (0, 0);
        }

        self.audio.snapshot(&mut self.samples);

        self.calibrate_latency();

        if let Some((_, t)) = &self.toast {
            if t.elapsed() > Duration::from_millis(1400) {
                self.toast = None;
            }
        }
    }

    /// Measure real audio-output latency from playback transients. When playback
    /// starts at a known song-position (resume from pause, or a fresh track) the
    /// first audible sound arrives one pipeline-latency later — by which time the
    /// player position has advanced exactly that much. So
    /// `latency = position_at_first_sound − start_position`. Correctly signed and
    /// independent of the lyrics, unlike content correlation.
    fn calibrate_latency(&mut self) {
        let playing = self.np.status == crate::player::Status::Playing;

        // Loudness with hysteresis → a clean silence→sound rising edge.
        let rms = if self.samples.is_empty() {
            0.0
        } else {
            let s: f32 = self.samples.iter().map(|x| x * x).sum();
            (s / self.samples.len() as f32).sqrt()
        };
        let was_loud = self.audio_loud;
        if rms > 0.030 {
            self.audio_loud = true;
        } else if rms < 0.008 {
            self.audio_loud = false;
        }
        let sound_started = !was_loud && self.audio_loud;

        let track_changed = self.np.title != self.cal_prev_title;
        self.cal_prev_title = self.np.title.clone();

        // Remember where we are while paused: that's the position playback will
        // resume from.
        if !playing {
            self.paused_pos = self.np.position;
        }

        match self.cal {
            Cal::Idle => {
                let resume = playing && !self.was_playing;
                // Arm only if the audio is currently silent, so we can catch the
                // onset. Track change starts at ~0; resume starts at paused_pos.
                let start_pos = if track_changed && !self.np.title.is_empty() {
                    Some(self.np.position)
                } else if resume {
                    Some(self.paused_pos)
                } else {
                    None
                };
                if let Some(sp) = start_pos {
                    if !self.audio_loud {
                        self.cal = Cal::Armed {
                            start_pos: sp,
                            since: Instant::now(),
                        };
                    }
                }
            }
            Cal::Armed { start_pos, since } => {
                if sound_started {
                    let l = self.np.position - start_pos;
                    if (0.02..=2.0).contains(&l) {
                        // Smooth toward the new measurement.
                        self.latency = if self.latency <= 0.0 {
                            l
                        } else {
                            self.latency * 0.5 + l * 0.5
                        };
                        self.cfg.lyrics_latency = self.latency;
                        self.toast(format!("latency calibrated: {:.0} ms", self.latency * 1000.0));
                    }
                    self.cal = Cal::Idle;
                } else if since.elapsed() > Duration::from_secs(3) {
                    self.cal = Cal::Idle; // no onset seen; give up this attempt
                }
            }
        }

        self.was_playing = playing;
    }

    /// (Re)build the album-art protocol for the current art area. Art keeps its
    /// native pixels for nearest-neighbour scaling. Only runs when the source or
    /// art-area size actually changes — never per frame.
    fn ensure_art_protocol(&mut self, song_area: Rect) {
        let Some(src) = &self.art_src else { return };
        let art = ui::art_rect(song_area);
        if art.width == 0 || art.height == 0 {
            return;
        }
        let dims = (art.width, art.height);
        if self.protocol.is_some() && self.proto_dims == dims {
            return;
        }

        self.protocol = Some(self.picker.new_resize_protocol(src.clone()));
        self.proto_dims = dims;
    }

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let song_w = song_panel_width(area.width);
        let cols = match self.cfg.song_side {
            Side::Left => Layout::horizontal([
                Constraint::Length(song_w),
                Constraint::Min(0),
            ])
            .split(area),
            Side::Right => Layout::horizontal([
                Constraint::Min(0),
                Constraint::Length(song_w),
            ])
            .split(area),
        };
        let (song_area, viz_area) = match self.cfg.song_side {
            Side::Left => (cols[0], cols[1]),
            Side::Right => (cols[1], cols[0]),
        };

        // Analyse audio sized to the visualizer's inner width.
        let inner_w = viz_area.width.saturating_sub(2);
        let want = self
            .cfg
            .mode
            .desired_bars(inner_w, self.cfg.bar_width, self.cfg.bar_gap);
        self.analyzer.set_bars(want);
        let dt = self.last_frame.elapsed().as_secs_f32().clamp(0.0, 0.1);
        self.last_frame = Instant::now();
        let levels = self.analyzer.compute(&self.samples, dt).to_vec();

        // Bass-energy envelope: average the lowest bands, snap up on a kick and
        // decay slowly, so the circle's ring throbs to the beat.
        if !levels.is_empty() {
            let bass_n = (levels.len() / 8).max(1);
            let bass = levels[..bass_n].iter().copied().sum::<f32>() / bass_n as f32;
            self.beat = if bass > self.beat {
                bass
            } else {
                (self.beat - 1.8 * dt).max(0.0)
            };
        }

        // Peak-hold: instant rise to the current level, slow fall.
        if self.cfg.mode == VisMode::Peaks {
            if self.peaks.len() != levels.len() {
                self.peaks = levels.clone();
            } else {
                let fall = 0.55 * dt;
                for (pk, &lv) in self.peaks.iter_mut().zip(&levels) {
                    *pk = if lv >= *pk { lv } else { (*pk - fall).max(lv) };
                }
            }
        }

        // Spectrogram: keep a rolling, width-bounded history of frames (only while
        // the mode is active; otherwise release the memory).
        if self.cfg.mode == VisMode::Spectro {
            self.spectro.push_back(levels.clone());
            let cap = inner_w.max(1) as usize;
            while self.spectro.len() > cap {
                self.spectro.pop_front();
            }
        } else if !self.spectro.is_empty() {
            self.spectro.clear();
        }

        // Wave mode: sample + temporally smooth the trace so it glides instead of
        // jumping frame-to-frame.
        if self.cfg.mode == VisMode::Wave {
            let target = visualizer::sample_wave(&self.samples, inner_w.max(1) as usize);
            if self.wave.len() != target.len() {
                self.wave = target;
            } else {
                for (w, t) in self.wave.iter_mut().zip(target) {
                    *w += (t - *w) * 0.55;
                }
            }
        }

        self.ensure_art_protocol(song_area);

        let elapsed = self.start.elapsed().as_secs_f64();
        self.hits = ui::render_song_panel(
            f,
            song_area,
            &self.np,
            &self.cfg,
            &mut self.protocol,
            self.drag_frac,
            elapsed,
        );

        // Lyrics follow audible output: subtract measured latency, plus the
        // optional manual trim.
        let effective = self.cfg.lyrics_offset - self.latency;
        let view = LyricsView {
            palette: self.cfg.palette,
            show_romaji: self.cfg.lyrics_romaji,
            show_translation: self.cfg.lyrics_translate,
            offset: effective,
        };
        let lyric_pos = self.np.position + effective;
        match self.cfg.lyrics_mode {
            LyricsMode::Off => self.render_visualizer(f, viz_area, &levels),
            LyricsMode::Full => {
                crate::lyrics_ui::render(f, viz_area, &self.lyrics_snapshot, lyric_pos, &view)
            }
            LyricsMode::Split => {
                let h = Layout::vertical([
                    Constraint::Percentage(55),
                    Constraint::Percentage(45),
                ])
                .split(viz_area);
                self.render_visualizer(f, h[0], &levels);
                crate::lyrics_ui::render(f, h[1], &self.lyrics_snapshot, lyric_pos, &view);
            }
        }

        match self.overlay {
            Overlay::None => {}
            Overlay::Help => render_help(f, area, self.cfg.palette),
            Overlay::Modes => self.render_mode_picker(f, area),
            Overlay::Palettes => self.render_palette_picker(f, area),
        }
    }

    fn render_visualizer(&self, f: &mut Frame, area: Rect, levels: &[f32]) {
        let title = match &self.toast {
            Some((msg, _)) => msg.clone(),
            None => format!(
                "{} · {}{}",
                self.cfg.mode.name(),
                self.cfg.palette.name(),
                if self.audio.is_connected() { "" } else { " · (no audio)" }
            ),
        };
        let params = VizParams {
            mode: self.cfg.mode,
            palette: self.cfg.palette,
            bar_width: self.cfg.bar_width,
            bar_gap: self.cfg.bar_gap,
            levels,
            wave: &self.wave,
            peaks: &self.peaks,
            time: self.start.elapsed().as_secs_f64(),
            beat: self.beat,
            spectro: &self.spectro,
            title,
        };
        visualizer::render(f, area, &params);
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if self.overlay != Overlay::None {
            self.on_key_overlay(key);
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }

            // transport
            KeyCode::Char('n') => self.player.send(Command::Next),
            KeyCode::Char('b') => self.player.send(Command::Prev),
            KeyCode::Char(' ') => self.player.send(Command::PlayPause),
            KeyCode::Right => self.player.send(Command::SeekForward),
            KeyCode::Left => self.player.send(Command::SeekBackward),
            KeyCode::Up => self.player.send(Command::VolumeUp),
            KeyCode::Down => self.player.send(Command::VolumeDown),

            // visualizer
            KeyCode::Char('m') => {
                self.overlay = Overlay::Modes;
                self.picker_saved = (self.cfg.mode, self.cfg.palette);
                self.picker_cursor = self.cfg.mode.index();
            }
            KeyCode::Char('c') => {
                self.overlay = Overlay::Palettes;
                self.picker_saved = (self.cfg.mode, self.cfg.palette);
                self.picker_cursor = self.cfg.palette.index();
            }
            KeyCode::Char('s') => {
                self.cfg.song_side = self.cfg.song_side.flip();
                self.toast("swapped sides".into());
            }
            KeyCode::Char('g') => {
                self.cfg.auto_gain = !self.cfg.auto_gain;
                self.analyzer.auto_gain = self.cfg.auto_gain;
                self.toast(format!(
                    "auto-gain: {}",
                    if self.cfg.auto_gain { "on" } else { "off" }
                ));
            }
            KeyCode::Char('+') | KeyCode::Char('=') => self.adjust_gain(1.15),
            KeyCode::Char('-') | KeyCode::Char('_') => self.adjust_gain(1.0 / 1.15),
            KeyCode::Char(']') => self.adjust_bar_width(1),
            KeyCode::Char('[') => self.adjust_bar_width(-1),

            // lyrics — cycle off → full → split → off
            KeyCode::Char('l') => {
                self.cfg.lyrics_mode = self.cfg.lyrics_mode.next();
                self.toast(format!("lyrics: {}", self.cfg.lyrics_mode.name()));
                if self.cfg.lyrics_mode.active() && !self.np.title.is_empty() {
                    self.request_lyrics();
                }
            }
            KeyCode::Char('t') => {
                self.cfg.lyrics_translate = !self.cfg.lyrics_translate;
                self.toast(format!(
                    "translation: {}",
                    if self.cfg.lyrics_translate { "on" } else { "off" }
                ));
                self.ensure_lyrics_extras(); // only fetches if not already loaded
            }
            KeyCode::Char('r') => {
                self.cfg.lyrics_romaji = !self.cfg.lyrics_romaji;
                self.toast(format!(
                    "pronunciation: {}",
                    if self.cfg.lyrics_romaji { "on" } else { "off" }
                ));
                self.ensure_lyrics_extras();
            }
            // lyrics sync nudge (compensate constant audio-output latency)
            KeyCode::Char('.') | KeyCode::Char('>') => self.adjust_lyrics_offset(0.1),
            KeyCode::Char(',') | KeyCode::Char('<') => self.adjust_lyrics_offset(-0.1),
            KeyCode::Char('w') => match self.cfg.save() {
                Ok(()) => self.toast("config saved".into()),
                Err(e) => self.toast(format!("save failed: {e}")),
            },
            KeyCode::Char('?') | KeyCode::Char('h') => self.overlay = Overlay::Help,
            _ => {}
        }
    }

    fn on_key_overlay(&mut self, key: KeyEvent) {
        // Help: any key dismisses (q still quits).
        if self.overlay == Overlay::Help {
            match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.should_quit = true
                }
                _ => self.overlay = Overlay::None,
            }
            return;
        }

        let is_modes = self.overlay == Overlay::Modes;
        let len = if is_modes {
            VisMode::ALL.len()
        } else {
            crate::theme::Palette::ALL.len()
        };
        match key.code {
            KeyCode::Esc => {
                // Revert the live preview to whatever was active when opened.
                self.cfg.mode = self.picker_saved.0;
                self.cfg.palette = self.picker_saved.1;
                self.overlay = Overlay::None;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.overlay = Overlay::None;
                if is_modes {
                    self.toast(format!("mode: {}", self.cfg.mode.name()));
                } else {
                    self.toast(format!("palette: {}", self.cfg.palette.name()));
                }
            }
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.picker_cursor = (self.picker_cursor + len - 1) % len;
                self.picker_apply();
            }
            KeyCode::Down
            | KeyCode::Char('j')
            | KeyCode::Tab
            | KeyCode::Char('m')
            | KeyCode::Char('c') => {
                self.picker_cursor = (self.picker_cursor + 1) % len;
                self.picker_apply();
            }
            _ => {}
        }
    }

    /// Apply the picker's current selection live (so the whole UI previews it).
    fn picker_apply(&mut self) {
        if self.overlay == Overlay::Modes {
            self.cfg.mode = VisMode::ALL[self.picker_cursor.min(VisMode::ALL.len() - 1)];
        } else if self.overlay == Overlay::Palettes {
            self.cfg.palette =
                crate::theme::Palette::ALL[self.picker_cursor.min(crate::theme::Palette::ALL.len() - 1)];
        }
    }

    fn on_mouse(&mut self, me: MouseEvent) {
        // While a picker is open, the wheel scrolls the selection (live preview)
        // and a click confirms it.
        if matches!(self.overlay, Overlay::Modes | Overlay::Palettes) {
            let len = if self.overlay == Overlay::Modes {
                VisMode::ALL.len()
            } else {
                crate::theme::Palette::ALL.len()
            };
            match me.kind {
                MouseEventKind::ScrollDown => {
                    self.picker_cursor = (self.picker_cursor + 1) % len;
                    self.picker_apply();
                }
                MouseEventKind::ScrollUp => {
                    self.picker_cursor = (self.picker_cursor + len - 1) % len;
                    self.picker_apply();
                }
                MouseEventKind::Down(MouseButton::Left) => self.overlay = Overlay::None,
                _ => {}
            }
            return;
        }
        if self.overlay == Overlay::Help {
            if let MouseEventKind::Down(MouseButton::Left) = me.kind {
                self.overlay = Overlay::None;
            }
            return;
        }

        let pos = Position::new(me.column, me.row);
        match me.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.hits.play_pause.contains(pos) {
                    self.player.send(Command::PlayPause);
                } else if self.hits.next.contains(pos) {
                    self.player.send(Command::Next);
                } else if self.hits.prev.contains(pos) {
                    self.player.send(Command::Prev);
                } else if self.hits.progress.contains(pos) {
                    self.dragging = true;
                    self.seek_to_column(me.column);
                } else if self.hits.volume.contains(pos) {
                    self.set_volume_at_column(me.column);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.dragging {
                    self.seek_to_column(me.column);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.dragging {
                    if let Some(frac) = self.drag_frac.take() {
                        self.player.send(Command::SeekTo(frac));
                    }
                    self.dragging = false;
                }
            }
            MouseEventKind::ScrollUp => self.player.send(Command::VolumeUp),
            MouseEventKind::ScrollDown => self.player.send(Command::VolumeDown),
            _ => {}
        }
    }

    fn seek_to_column(&mut self, col: u16) {
        let r = self.hits.progress;
        if r.width == 0 {
            return;
        }
        let frac = ((col.saturating_sub(r.x)) as f64 / r.width as f64).clamp(0.0, 1.0);
        self.drag_frac = Some(frac);
        // Stream updates live, but throttle the actual D-Bus seeks.
        if self.last_seek.elapsed() > Duration::from_millis(90) {
            self.player.send(Command::SeekTo(frac));
            self.last_seek = Instant::now();
        }
    }

    fn set_volume_at_column(&mut self, col: u16) {
        let r = self.hits.volume;
        if r.width == 0 {
            return;
        }
        // The bar has a "vol NNN% " label prefix; approximate by using the whole row.
        let frac = ((col.saturating_sub(r.x)) as f64 / r.width as f64).clamp(0.0, 1.0);
        self.player.send(Command::SetVolume(frac));
    }

    fn adjust_gain(&mut self, factor: f32) {
        self.cfg.gain = (self.cfg.gain * factor).clamp(0.1, 12.0);
        self.analyzer.gain = self.cfg.gain;
        self.toast(format!("gain: {:.2}", self.cfg.gain));
    }

    fn adjust_lyrics_offset(&mut self, delta: f64) {
        self.cfg.lyrics_offset = (self.cfg.lyrics_offset + delta).clamp(-10.0, 10.0);
        self.toast(format!("lyrics sync: {:+.2}s", self.cfg.lyrics_offset));
    }

    fn adjust_bar_width(&mut self, delta: i32) {
        let w = (self.cfg.bar_width as i32 + delta).clamp(1, 8) as u16;
        self.cfg.bar_width = w;
        self.toast(format!("bar width: {w}"));
    }

    fn toast(&mut self, msg: String) {
        self.toast = Some((msg, Instant::now()));
    }

    fn render_mode_picker(&self, f: &mut Frame, area: Rect) {
        let items: Vec<PickerItem> = VisMode::ALL
            .iter()
            .map(|m| PickerItem {
                name: m.name().to_string(),
                blurb: m.blurb().to_string(),
                palette: None,
            })
            .collect();
        render_picker(f, area, " visualizer mode ", self.picker_cursor, &items, self.cfg.palette);
    }

    fn render_palette_picker(&self, f: &mut Frame, area: Rect) {
        let items: Vec<PickerItem> = crate::theme::Palette::ALL
            .iter()
            .map(|p| PickerItem {
                name: p.name().to_string(),
                blurb: String::new(),
                palette: Some(*p),
            })
            .collect();
        render_picker(f, area, " color palette ", self.picker_cursor, &items, self.cfg.palette);
    }
}

struct PickerItem {
    name: String,
    blurb: String,
    palette: Option<crate::theme::Palette>,
}

/// A centered, scrollable selection overlay with live-preview styling. The
/// border/title pick up the current palette so the theme reads everywhere.
fn render_picker(
    f: &mut Frame,
    area: Rect,
    title: &str,
    cursor: usize,
    items: &[PickerItem],
    accent_pal: crate::theme::Palette,
) {
    let accent = accent_pal.accent();
    let muted = Color::Rgb(120, 120, 135);

    let w = 56u16.min(area.width.saturating_sub(4)).max(20);
    let visible = items
        .len()
        .min((area.height as usize).saturating_sub(6).max(3));
    let h = ((visible as u16) + 3).min(area.height.saturating_sub(2)); // 2 border + 1 hint
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);

    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .title(Span::styled(
            title.to_string(),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 2 {
        return;
    }

    let win = inner.height.saturating_sub(1) as usize; // reserve last row for hint
    // Scroll so the cursor stays roughly centered and in view.
    let start = if items.len() <= win {
        0
    } else if cursor < win / 2 {
        0
    } else if cursor + win / 2 >= items.len() {
        items.len() - win
    } else {
        cursor - win / 2
    };

    let mut lines: Vec<Line> = Vec::new();
    for (i, item) in items.iter().enumerate().skip(start).take(win) {
        let selected = i == cursor;
        let mut spans = vec![Span::styled(
            if selected { "▸ " } else { "  " },
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )];
        if let Some(p) = item.palette {
            for k in 0..12 {
                let t = k as f32 / 11.0;
                spans.push(Span::styled("█", Style::default().fg(p.color(t))));
            }
            spans.push(Span::raw(" "));
        }
        let name_style = if selected {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Rgb(180, 180, 195))
        };
        spans.push(Span::styled(format!("{:<10}", item.name), name_style));
        if !item.blurb.is_empty() {
            spans.push(Span::styled(item.blurb.clone(), Style::default().fg(muted)));
        }
        lines.push(Line::from(spans));
    }

    let list_rect = Rect::new(inner.x, inner.y, inner.width, win as u16);
    f.render_widget(Paragraph::new(lines), list_rect);
    let hint_rect = Rect::new(inner.x, inner.y + win as u16, inner.width, 1);
    f.render_widget(
        Paragraph::new(Span::styled(
            "↑↓ select · enter apply · esc cancel",
            Style::default().fg(muted),
        )),
        hint_rect,
    );
}

/// Song panel width: ~40% of the screen, clamped so album art stays large but
/// the visualizer still gets the lion's share.
fn song_panel_width(total: u16) -> u16 {
    let w = (total as f32 * 0.40) as u16;
    w.clamp(40, 70).min(total.saturating_sub(12))
}

fn render_help(f: &mut Frame, area: Rect, palette: crate::theme::Palette) {
    let accent = palette.accent();
    let accent_dim = palette.accent_dim();
    // Two-column layout so the menu stays short and never gets clipped.
    let binds: [(&str, &str); 17] = [
        ("n / b", "next / prev track"),
        ("space", "play / pause"),
        ("← / →", "seek 5s"),
        ("↑ / ↓", "volume"),
        ("m", "mode picker"),
        ("c", "palette picker"),
        ("s", "swap sides"),
        ("+ / -", "gain"),
        ("[ / ]", "bar width"),
        ("g", "toggle auto-gain"),
        ("l", "lyrics off/full/split"),
        ("t", "lyrics translation"),
        ("r", "lyrics pronunciation"),
        (", / .", "lyrics sync -/+"),
        ("w", "save config"),
        ("? / h", "toggle help"),
        ("q / esc", "quit"),
    ];

    let mut lines: Vec<Line> = Vec::new();
    let cell = |k: &str, d: &str| -> Vec<Span<'static>> {
        vec![
            Span::styled(
                format!("{k:>7} ", k = k),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{d:<18}", d = d),
                Style::default().fg(Color::Rgb(205, 205, 215)),
            ),
        ]
    };
    for pair in binds.chunks(2) {
        let mut spans = cell(pair[0].0, pair[0].1);
        if let Some(second) = pair.get(1) {
            spans.push(Span::raw("  "));
            spans.extend(cell(second.0, second.1));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " mouse: click buttons · drag bar to seek",
        Style::default().fg(Color::Rgb(140, 140, 155)),
    )));
    lines.push(Line::from(Span::styled(
        " scroll = volume",
        Style::default().fg(Color::Rgb(140, 140, 155)),
    )));

    let content_h = lines.len() as u16 + 2; // borders
    let w = 64u16.min(area.width.saturating_sub(4));
    let h = content_h.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);

    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent_dim))
        .title(Span::styled(
            " keybindings ",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}
