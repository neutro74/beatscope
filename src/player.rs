//! Now-playing metadata and transport controls via MPRIS (D-Bus).
//!
//! A dedicated thread owns the MPRIS `Player` (which is not `Send`-friendly to
//! share), polls metadata into a shared snapshot, and applies control commands
//! received over a channel.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mpris::{PlaybackStatus, PlayerFinder};

#[derive(Clone, Debug, Default)]
pub struct NowPlaying {
    pub connected: bool,
    pub player_name: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: Option<String>,
    pub position: f64,
    pub length: f64,
    pub status: Status,
    pub volume: f64,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    Playing,
    Paused,
    #[default]
    Stopped,
}

pub enum Command {
    Next,
    Prev,
    PlayPause,
    SeekForward,
    SeekBackward,
    /// Seek to an absolute fraction (0..1) of the current track.
    SeekTo(f64),
    VolumeUp,
    VolumeDown,
    /// Set absolute volume (0..1).
    SetVolume(f64),
}

/// Shared player state plus the clock anchor used to interpolate playback
/// position smoothly between the (relatively infrequent) MPRIS polls.
struct Shared {
    np: NowPlaying,
    /// Position (seconds) sampled at `anchor_at`.
    anchor_pos: f64,
    anchor_at: Instant,
    playing: bool,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            np: NowPlaying::default(),
            anchor_pos: 0.0,
            anchor_at: Instant::now(),
            playing: false,
        }
    }
}

pub struct PlayerHandle {
    state: Arc<Mutex<Shared>>,
    tx: Sender<Command>,
}

impl PlayerHandle {
    pub fn start() -> PlayerHandle {
        let state = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = mpsc::channel();
        let st = state.clone();
        thread::spawn(move || player_thread(st, rx));
        PlayerHandle { state, tx }
    }

    /// A current snapshot with the playback position advanced by a local clock,
    /// so lyrics and the seeker stay smooth and accurate between MPRIS polls.
    pub fn snapshot(&self) -> NowPlaying {
        let s = self.state.lock().unwrap();
        let mut np = s.np.clone();
        let live = if s.playing {
            s.anchor_pos + s.anchor_at.elapsed().as_secs_f64()
        } else {
            s.anchor_pos
        };
        // Don't clamp to the reported length: some players report a wrong/short
        // (or stale) length, and clamping would freeze the position (and lyrics)
        // and pin the progress bar at 100% even though playback continues.
        np.position = live.max(0.0);
        np
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

fn player_thread(state: Arc<Mutex<Shared>>, rx: Receiver<Command>) {
    let finder = match PlayerFinder::new() {
        Ok(f) => f,
        Err(_) => return,
    };
    let mut player: Option<mpris::Player> = None;
    let mut last = NowPlaying::default();
    // Clock anchor for interpolation. Re-anchored whenever the real reading
    // diverges from our prediction (a seek, a pause, or a track change).
    let mut anchor_pos = 0.0f64;
    let mut anchor_at = Instant::now();
    let mut prev_title = String::new();
    let mut prev_len = 0.0f64;
    let mut prev_playing = false;
    let mut diverge_count = 0u8;

    loop {
        // Acquire / re-acquire a player if needed.
        if player.as_ref().map(|p| !p.is_running()).unwrap_or(true) {
            player = finder.find_active().ok();
            if player.is_none() {
                last = NowPlaying::default();
            }
        }

        // Refresh the snapshot, falling back to the last good values for any
        // field that momentarily fails (common during track transitions).
        if let Some(p) = &player {
            last = read_now_playing(p, &last);
        }

        // Free-running real-time clock. Playback and our clock both advance at
        // real time, so once anchored at track start there's nothing to keep
        // correcting — and nothing to jump. This player's MPRIS position is
        // blippy (it momentarily resets mid-transition), so we ignore transient
        // divergence and only re-anchor on a *confirmed* seek: several polls in a
        // row that disagree. Track change / pause re-anchor immediately.
        let now = Instant::now();
        let playing = last.status == Status::Playing;
        let track_changed = last.title != prev_title || (last.length - prev_len).abs() > 0.5;
        prev_title = last.title.clone();
        prev_len = last.length;

        let resumed = playing && !prev_playing;
        prev_playing = playing;
        if track_changed || !playing || resumed {
            // Anchor cleanly on pause, track change, and the moment playback
            // resumes — so the clock restarts exactly at the resume position
            // (no leftover poll-interval bias).
            anchor_pos = last.position;
            anchor_at = now;
            diverge_count = 0;
        } else {
            let predicted = anchor_pos + (now - anchor_at).as_secs_f64();
            let diff = last.position - predicted;
            if diff.abs() > 0.7 {
                diverge_count = diverge_count.saturating_add(1);
                if diverge_count >= 3 {
                    // Confirmed seek — re-anchor to the real position.
                    anchor_pos = last.position;
                    anchor_at = now;
                    diverge_count = 0;
                }
                // else: hold the free-running clock (transient blip).
            } else {
                diverge_count = 0;
                // In agreement: keep free-running (do not touch the anchor).
            }
        }

        {
            let mut s = state.lock().unwrap();
            s.np = last.clone();
            s.anchor_pos = anchor_pos;
            s.anchor_at = anchor_at;
            s.playing = playing;
        }

        // Respond to commands promptly; otherwise poll on a short interval.
        match rx.recv_timeout(Duration::from_millis(120)) {
            Ok(cmd) => {
                if let Some(p) = &player {
                    apply(p, cmd);
                    while let Ok(c) = rx.try_recv() {
                        apply(p, c);
                    }
                }
                // Loop straight back to refresh so the UI reflects the change fast.
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn apply(p: &mpris::Player, cmd: Command) {
    match cmd {
        Command::Next => {
            let _ = p.next();
        }
        Command::Prev => {
            let _ = p.previous();
        }
        Command::PlayPause => {
            let _ = p.play_pause();
        }
        Command::SeekForward => {
            let _ = p.seek_forwards(&Duration::from_secs(5));
        }
        Command::SeekBackward => {
            let _ = p.seek_backwards(&Duration::from_secs(5));
        }
        Command::SeekTo(frac) => {
            if let Ok(meta) = p.get_metadata() {
                if let (Some(tid), Some(len)) = (meta.track_id(), meta.length()) {
                    let pos = Duration::from_secs_f64(frac.clamp(0.0, 1.0) * len.as_secs_f64());
                    let _ = p.set_position(tid, &pos);
                }
            }
        }
        Command::VolumeUp => {
            if let Ok(v) = p.get_volume() {
                let _ = p.set_volume((v + 0.05).min(1.0));
            }
        }
        Command::VolumeDown => {
            if let Ok(v) = p.get_volume() {
                let _ = p.set_volume((v - 0.05).max(0.0));
            }
        }
        Command::SetVolume(v) => {
            let _ = p.set_volume(v.clamp(0.0, 1.0));
        }
    }
}

fn read_now_playing(p: &mpris::Player, last: &NowPlaying) -> NowPlaying {
    let status = match p.get_playback_status() {
        Ok(PlaybackStatus::Playing) => Status::Playing,
        Ok(PlaybackStatus::Paused) => Status::Paused,
        Ok(PlaybackStatus::Stopped) => Status::Stopped,
        Err(_) => last.status,
    };
    let volume = p.get_volume().unwrap_or(last.volume);

    // Metadata can briefly be empty mid-transition; keep the last good values
    // instead of flashing to "nothing playing" (which corrupts the seeker/art).
    let (title, artist, album, art_url, length) = match p.get_metadata() {
        Ok(m) => {
            let title = m.title().unwrap_or("").to_string();
            if title.is_empty() {
                (
                    last.title.clone(),
                    last.artist.clone(),
                    last.album.clone(),
                    last.art_url.clone(),
                    last.length,
                )
            } else {
                (
                    title,
                    m.artists()
                        .map(|a| a.join(", "))
                        .filter(|s| !s.is_empty())
                        .unwrap_or_default(),
                    m.album_name().unwrap_or("").to_string(),
                    m.art_url().map(str::to_string),
                    m.length().map(|d| d.as_secs_f64()).unwrap_or(0.0),
                )
            }
        }
        Err(_) => (
            last.title.clone(),
            last.artist.clone(),
            last.album.clone(),
            last.art_url.clone(),
            last.length,
        ),
    };

    // Keep the last length only while it's the *same* track (covers a momentary
    // empty read); a new track with an unknown length must not inherit the old
    // one — that's how a 7-minute song ends up showing a previous 2:14 length.
    let length = if length > 0.0 {
        length
    } else if title == last.title {
        last.length
    } else {
        0.0
    };
    let position = p.get_position().map(|d| d.as_secs_f64()).unwrap_or(last.position);

    NowPlaying {
        connected: true,
        player_name: p.identity().to_string(),
        title,
        artist,
        album,
        art_url,
        position,
        length,
        status,
        volume,
    }
}
