//! On-disk cache for fetched lyrics and converged sync offsets, so replaying a
//! track is instant and doesn't re-hit the network.
//!
//! Lives under the user's cache dir (`~/.cache/beatscope/`). Lyrics are stored
//! one JSON file per track (keyed by a hash of artist/title/album/duration);
//! offsets share a single small JSON map.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::lyrics::LyricLine;

/// A cached lyrics result plus which extras were materialised when it was saved,
/// so we can tell whether a later request needs additional network work.
#[derive(Clone, Serialize, Deserialize)]
pub struct CachedLyrics {
    pub lines: Vec<LyricLine>,
    pub had_translate: bool,
    pub had_pronounce: bool,
}

fn base_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("beatscope"))
}

/// Stable per-track key. Album + rounded duration disambiguate re-recordings.
pub fn track_key(artist: &str, title: &str, album: &str, duration: f64) -> String {
    format!(
        "{}|{}|{}|{}",
        artist.trim().to_lowercase(),
        title.trim().to_lowercase(),
        album.trim().to_lowercase(),
        duration.round() as i64
    )
}

/// FNV-1a 64-bit — a tiny, dependency-free hash for filesystem-safe filenames.
fn hash_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x00000100000001B3);
    }
    format!("{h:016x}")
}

/// Bump when the cached lyric structure changes so stale files are ignored.
const LYRICS_CACHE_VER: u32 = 3;

fn lyrics_path(key: &str) -> Option<PathBuf> {
    Some(base_dir()?.join("lyrics").join(format!(
        "{}-v{}.json",
        hash_hex(key),
        LYRICS_CACHE_VER
    )))
}

pub fn load_lyrics(key: &str) -> Option<CachedLyrics> {
    let path = lyrics_path(key)?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_lyrics(key: &str, data: &CachedLyrics) {
    let Some(path) = lyrics_path(key) else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(data) {
        let _ = std::fs::write(path, text);
    }
}

