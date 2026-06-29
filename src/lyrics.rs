//! Synced lyrics: fetch (lrclib.net), LRC parsing, rōmaji, and optional
//! line-by-line translation (unofficial Google endpoint), all off the UI thread.

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cache::{self, CachedLyrics};
use crate::romaji;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct LyricLine {
    pub time: f64,
    pub text: String,
    pub romaji: Option<String>,
    pub translation: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Loading,
    Loaded,
    Unsynced,
    NotFound,
    Error,
}

#[derive(Clone)]
pub struct Lyrics {
    pub key: String,
    pub status: Status,
    pub lines: Vec<LyricLine>,
}

impl Default for Lyrics {
    fn default() -> Self {
        Lyrics {
            key: String::new(),
            status: Status::Idle,
            lines: Vec::new(),
        }
    }
}

impl Lyrics {
    /// Index of the active line for a playback position (last line with time<=pos).
    pub fn active_index(&self, pos: f64) -> Option<usize> {
        if self.lines.is_empty() {
            return None;
        }
        let mut idx = None;
        for (i, l) in self.lines.iter().enumerate() {
            if l.time <= pos + 0.01 {
                idx = Some(i);
            } else {
                break;
            }
        }
        idx.or(Some(0))
    }

    /// Approximate the active word within a line by spreading the line's duration
    /// across its words proportionally to length.
    pub fn active_word(&self, line_idx: usize, pos: f64) -> Option<usize> {
        let line = self.lines.get(line_idx)?;
        let start = line.time;
        let end = self
            .lines
            .get(line_idx + 1)
            .map(|l| l.time)
            .unwrap_or(start + 4.0);
        let span = (end - start).max(0.1);
        let words: Vec<&str> = line.text.split_whitespace().collect();
        if words.len() <= 1 {
            return if words.is_empty() { None } else { Some(0) };
        }
        let total: usize = words.iter().map(|w| w.chars().count().max(1)).sum();
        let frac = ((pos - start) / span).clamp(0.0, 0.999);
        let target = frac * total as f64;
        let mut acc = 0.0;
        for (i, w) in words.iter().enumerate() {
            acc += w.chars().count().max(1) as f64;
            if target < acc {
                return Some(i);
            }
        }
        Some(words.len() - 1)
    }
}

pub struct Request {
    pub key: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration: f64,
    pub translate: bool,
    /// Fetch network romanization (covers kanji and any script, unlike the
    /// instant offline kana fallback).
    pub pronounce: bool,
}

pub struct LyricsManager {
    state: Arc<Mutex<Lyrics>>,
    tx: Sender<Request>,
    generation: Arc<AtomicU64>,
}

impl LyricsManager {
    pub fn start() -> LyricsManager {
        let state = Arc::new(Mutex::new(Lyrics::default()));
        let generation = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel();
        let st = state.clone();
        let gener = generation.clone();
        thread::spawn(move || worker(st, gener, rx));
        LyricsManager {
            state,
            tx,
            generation,
        }
    }

    pub fn request(&self, req: Request) {
        // Bump generation so the worker abandons any in-flight translation work.
        self.generation.fetch_add(1, Ordering::SeqCst);
        let _ = self.tx.send(req);
    }

    pub fn snapshot(&self) -> Lyrics {
        self.state.lock().unwrap().clone()
    }
}

fn worker(state: Arc<Mutex<Lyrics>>, generation: Arc<AtomicU64>, rx: Receiver<Request>) {
    while let Ok(mut req) = rx.recv() {
        // Coalesce to the most recent request.
        while let Ok(newer) = rx.try_recv() {
            req = newer;
        }
        let my_gen = generation.load(Ordering::SeqCst);

        {
            let mut s = state.lock().unwrap();
            s.key = req.key.clone();
            s.status = Status::Loading;
            s.lines.clear();
        }

        let cache_key = cache::track_key(&req.artist, &req.title, &req.album, req.duration);

        // Cache hit: publish instantly, no base network fetch.
        if let Some(cached) = cache::load_lyrics(&cache_key) {
            let mut lines = cached.lines;
            let synced = lines.iter().any(|l| l.time > 0.0) || lines.len() > 1;
            if generation.load(Ordering::SeqCst) != my_gen {
                continue;
            }
            {
                let mut s = state.lock().unwrap();
                s.lines = lines.clone();
                s.status = if synced { Status::Loaded } else { Status::Unsynced };
            }
            // Only the still-missing extras need the network.
            let need_tr = req.translate && !cached.had_translate;
            let need_rm = req.pronounce && !cached.had_pronounce;
            if need_tr || need_rm {
                let done = run_extras(&mut lines, need_tr, need_rm, &state, &req.key, my_gen, &generation);
                if done {
                    cache::save_lyrics(
                        &cache_key,
                        &CachedLyrics {
                            lines,
                            had_translate: cached.had_translate || req.translate,
                            had_pronounce: cached.had_pronounce || req.pronounce,
                        },
                    );
                }
            }
            continue;
        }

        let result = fetch_lyrics(&req);
        let mut lines = match result {
            Ok(Some(mut lines)) => {
                // Instant offline pronunciation only for clean (pure-kana) lines;
                // kanji/other-script lines are filled later by the network reading.
                for l in &mut lines {
                    l.romaji = romaji::romanize_if_clean(&l.text);
                }
                lines
            }
            Ok(None) => {
                set_status(&state, &req.key, my_gen, &generation, Status::NotFound);
                continue;
            }
            Err(_) => {
                set_status(&state, &req.key, my_gen, &generation, Status::Error);
                continue;
            }
        };

        let synced = lines.iter().any(|l| l.time > 0.0) || lines.len() > 1;
        {
            if generation.load(Ordering::SeqCst) != my_gen {
                continue;
            }
            let mut s = state.lock().unwrap();
            s.lines = lines.clone();
            s.status = if synced { Status::Loaded } else { Status::Unsynced };
        }
        // Cache the base result right away so a quick replay is instant even if
        // the extras pass below is interrupted.
        cache::save_lyrics(
            &cache_key,
            &CachedLyrics {
                lines: lines.clone(),
                had_translate: false,
                had_pronounce: false,
            },
        );

        // Fetch translation and/or full romanization line-by-line in the
        // background (one request per line covers both), publishing as we go.
        if req.translate || req.pronounce {
            let done = run_extras(
                &mut lines,
                req.translate,
                req.pronounce,
                &state,
                &req.key,
                my_gen,
                &generation,
            );
            if done {
                cache::save_lyrics(
                    &cache_key,
                    &CachedLyrics {
                        lines,
                        had_translate: req.translate,
                        had_pronounce: req.pronounce,
                    },
                );
            }
        }
    }
}

/// Number of concurrent line fetches. Enough to hide per-request latency without
/// hammering the (single-host) translation endpoint into rate limiting.
const EXTRAS_WORKERS: usize = 6;

/// Fetch translation and/or romanization for the lines still missing them,
/// publishing each as it arrives. Runs the per-line requests concurrently over a
/// small worker pool (sharing the keep-alive agent), which is dramatically faster
/// than one blocking round-trip at a time. Returns false if a newer request
/// superseded this one mid-way (so the caller shouldn't cache a partial result as
/// complete).
fn run_extras(
    lines: &mut [LyricLine],
    want_tr: bool,
    want_rm: bool,
    state: &Arc<Mutex<Lyrics>>,
    key: &str,
    my_gen: u64,
    generation: &Arc<AtomicU64>,
) -> bool {
    // Lines that still need a network fetch (text + what's missing).
    let tasks: Vec<(usize, String, bool, bool)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            if l.text.trim().is_empty() {
                return None;
            }
            let need_tr = want_tr && l.translation.is_none();
            // No point romanizing a line that's already plain ASCII (e.g. lyrics
            // that already come romanized) — it would just duplicate the line.
            let need_rm = want_rm && l.romaji.is_none() && !l.text.is_ascii();
            (need_tr || need_rm).then(|| (i, l.text.clone(), need_tr, need_rm))
        })
        .collect();
    if tasks.is_empty() {
        return true;
    }

    let tasks = Arc::new(tasks);
    let next = Arc::new(AtomicUsize::new(0));
    let cancelled = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<(usize, Option<String>, Option<String>)>();

    let workers = tasks.len().min(EXTRAS_WORKERS).max(1);
    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let tasks = tasks.clone();
        let next = next.clone();
        let cancelled = cancelled.clone();
        let state = state.clone();
        let generation = generation.clone();
        let tx = tx.clone();
        let key = key.to_string();
        handles.push(thread::spawn(move || loop {
            if generation.load(Ordering::SeqCst) != my_gen {
                cancelled.store(true, Ordering::SeqCst);
                return;
            }
            let idx = next.fetch_add(1, Ordering::SeqCst);
            let Some((line_i, text, need_tr, need_rm)) = tasks.get(idx) else {
                return;
            };
            let (tr, rm) = fetch_line(text, *need_tr, *need_rm);
            if generation.load(Ordering::SeqCst) != my_gen {
                cancelled.store(true, Ordering::SeqCst);
                return;
            }
            // Publish live into the displayed snapshot so lines fill in as they
            // resolve. The network reading replaces any kana fallback.
            {
                let mut s = state.lock().unwrap();
                if s.key == key {
                    if let Some(line) = s.lines.get_mut(*line_i) {
                        if tr.is_some() {
                            line.translation = tr.clone();
                        }
                        if rm.is_some() {
                            line.romaji = rm.clone();
                        }
                    }
                }
            }
            let _ = tx.send((*line_i, tr, rm));
        }));
    }
    drop(tx);

    // Fold results back into the local copy so the caller can cache them.
    for (i, tr, rm) in rx {
        if tr.is_some() {
            lines[i].translation = tr;
        }
        if rm.is_some() {
            lines[i].romaji = rm;
        }
    }
    for h in handles {
        let _ = h.join();
    }
    !cancelled.load(Ordering::SeqCst)
}

fn set_status(
    state: &Arc<Mutex<Lyrics>>,
    key: &str,
    my_gen: u64,
    generation: &Arc<AtomicU64>,
    status: Status,
) {
    if generation.load(Ordering::SeqCst) != my_gen {
        return;
    }
    let mut s = state.lock().unwrap();
    if s.key == key {
        s.status = status;
    }
}

fn is_synced(lines: &[LyricLine]) -> bool {
    lines.iter().any(|l| l.time > 0.0)
}

/// Try each provider in order. Return the first *synced* result; otherwise the
/// first plain result as a fallback. This is what lets newer tracks resolve even
/// when the strict lrclib match misses.
fn fetch_lyrics(req: &Request) -> Result<Option<Vec<LyricLine>>, String> {
    type Provider = fn(&Request) -> Option<Vec<LyricLine>>;
    let providers: [Provider; 3] = [lrclib_get, lrclib_search, netease];

    let mut fallback: Option<Vec<LyricLine>> = None;
    for provider in providers {
        if let Some(lines) = provider(req) {
            if lines.is_empty() {
                continue;
            }
            if is_synced(&lines) {
                return Ok(Some(lines));
            }
            fallback.get_or_insert(lines);
        }
    }
    Ok(fallback)
}

fn plain_lines(text: &str) -> Vec<LyricLine> {
    text.lines()
        .map(|t| LyricLine {
            time: -1.0,
            text: t.trim().to_string(),
            ..Default::default()
        })
        .collect()
}

fn lyrics_from_lrclib_obj(v: &serde_json::Value) -> Option<Vec<LyricLine>> {
    if let Some(s) = v.get("syncedLyrics").and_then(|v| v.as_str()) {
        if !s.trim().is_empty() {
            return Some(parse_lrc(s));
        }
    }
    if let Some(p) = v.get("plainLyrics").and_then(|v| v.as_str()) {
        if !p.trim().is_empty() {
            return Some(plain_lines(p));
        }
    }
    None
}

/// Exact match — fast and authoritative when album/duration line up.
fn lrclib_get(req: &Request) -> Option<Vec<LyricLine>> {
    let url = format!(
        "https://lrclib.net/api/get?artist_name={}&track_name={}&album_name={}&duration={}",
        url_encode(&req.artist),
        url_encode(&req.title),
        url_encode(&req.album),
        req.duration.round() as i64
    );
    let body = http_get(&url).ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    lyrics_from_lrclib_obj(&json)
}

/// Fuzzy search — catches tracks whose album/duration don't match the exact get.
fn lrclib_search(req: &Request) -> Option<Vec<LyricLine>> {
    let url = format!(
        "https://lrclib.net/api/search?track_name={}&artist_name={}",
        url_encode(&req.title),
        url_encode(&req.artist),
    );
    let body = http_get(&url).ok()?;
    let arr: Vec<serde_json::Value> = serde_json::from_str(&body).ok()?;
    let best = arr.iter().max_by(|a, b| {
        score_candidate(
            a.get("trackName").and_then(|v| v.as_str()).unwrap_or(""),
            a.get("artistName").and_then(|v| v.as_str()).unwrap_or(""),
            a.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0),
            a.get("syncedLyrics").and_then(|v| v.as_str()).is_some(),
            req,
        )
        .total_cmp(&score_candidate(
            b.get("trackName").and_then(|v| v.as_str()).unwrap_or(""),
            b.get("artistName").and_then(|v| v.as_str()).unwrap_or(""),
            b.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0),
            b.get("syncedLyrics").and_then(|v| v.as_str()).is_some(),
            req,
        ))
    })?;
    lyrics_from_lrclib_obj(best)
}

/// NetEase — broad catalogue (incl. covers and a lot of Asian music).
fn netease(req: &Request) -> Option<Vec<LyricLine>> {
    let q = format!("{} {}", req.title, req.artist);
    let url = format!(
        "https://music.163.com/api/search/get?s={}&type=1&limit=5",
        url_encode(&q)
    );
    let body = http_get_ref(&url, Some("https://music.163.com")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let songs = json.get("result")?.get("songs")?.as_array()?;

    let best = songs.iter().max_by(|a, b| {
        let artist = |x: &serde_json::Value| {
            x.get("artists")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|a| a.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        score_candidate(
            a.get("name").and_then(|v| v.as_str()).unwrap_or(""),
            &artist(a),
            a.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0) / 1000.0,
            true,
            req,
        )
        .total_cmp(&score_candidate(
            b.get("name").and_then(|v| v.as_str()).unwrap_or(""),
            &artist(b),
            b.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0) / 1000.0,
            true,
            req,
        ))
    })?;
    let id = best.get("id")?.as_i64()?;

    let url = format!("https://music.163.com/api/song/lyric?id={id}&lv=1&kv=1&tv=-1");
    let body = http_get_ref(&url, Some("https://music.163.com")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let lrc = json.get("lrc")?.get("lyric")?.as_str()?;
    if lrc.trim().is_empty() {
        return None;
    }
    Some(parse_lrc(lrc))
}

/// Heuristic match score: title/artist similarity, duration closeness, synced bonus.
fn score_candidate(title: &str, artist: &str, dur: f64, synced: bool, req: &Request) -> f64 {
    let norm = |s: &str| s.to_lowercase();
    let (t, a) = (norm(title), norm(artist));
    let (wt, wa) = (norm(&req.title), norm(&req.artist));
    let mut score = 0.0;

    if t == wt {
        score += 3.0;
    } else if t.contains(&wt) || wt.contains(&t) {
        score += 1.5;
    }
    if !wa.is_empty() && (a.contains(&wa) || wa.contains(&a)) {
        score += 1.5;
    }
    if req.duration > 0.0 && dur > 0.0 {
        let diff = (dur - req.duration).abs();
        score += (3.0 - diff).max(-3.0); // within 3s is a strong signal
    }
    if synced {
        score += 2.0;
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc_timestamps() {
        let lrc = "[00:15.21] hello\n[01:05.50] world\n[ar:Someone]";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 2);
        assert!((lines[0].time - 15.21).abs() < 0.001);
        assert_eq!(lines[0].text, "hello");
        assert!((lines[1].time - 65.50).abs() < 0.001);
    }

    #[test]
    fn merges_same_timestamp_bilingual() {
        // Original and translation share a timestamp → one line, translation set.
        let lrc = "[00:10.00] 日本語\n[00:10.00] Japanese\n[00:20.00] 次\n[00:20.00] Next";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 2, "doubled lines should be merged");
        assert_eq!(lines[0].text, "日本語");
        assert_eq!(lines[0].translation.as_deref(), Some("Japanese"));
        assert_eq!(lines[1].text, "次");
        assert_eq!(lines[1].translation.as_deref(), Some("Next"));
    }

    #[test]
    fn splits_inline_translation_when_pervasive() {
        let lrc = "[00:01.00] Boku no (My)\n[00:02.00] Kimi ni (To you)\n[00:03.00] Yoru ga (Night)\n[00:04.00] Sore made (Until then)";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].text, "Boku no");
        assert_eq!(lines[0].translation.as_deref(), Some("My"));
        assert_eq!(lines[3].text, "Sore made");
        assert_eq!(lines[3].translation.as_deref(), Some("Until then"));
    }

    #[test]
    fn keeps_occasional_parenthetical_adlib() {
        // Only one of several lines has a parenthetical — leave them all alone.
        let lrc = "[00:01.00] hello world\n[00:02.00] singing (oh)\n[00:03.00] another line\n[00:04.00] last line";
        let lines = parse_lrc(lrc);
        assert_eq!(lines[1].text, "singing (oh)");
        assert!(lines[1].translation.is_none());
    }

    #[test]
    fn active_index_tracks_position() {
        let lines = vec![
            LyricLine { time: 0.0, text: "a".into(), ..Default::default() },
            LyricLine { time: 10.0, text: "b".into(), ..Default::default() },
            LyricLine { time: 20.0, text: "c".into(), ..Default::default() },
        ];
        let l = Lyrics { key: String::new(), status: Status::Loaded, lines };
        assert_eq!(l.active_index(5.0), Some(0));
        assert_eq!(l.active_index(15.0), Some(1));
        assert_eq!(l.active_index(25.0), Some(2));
    }

    #[test]
    fn url_encodes_japanese() {
        assert_eq!(url_encode("a b"), "a%20b");
        assert!(url_encode("彼").starts_with('%'));
    }

    #[test]
    #[ignore = "hits the network"]
    fn live_looping_the_rooms() {
        let req = Request {
            key: "k".into(),
            artist: "rusino".into(),
            title: "Looping the Rooms".into(),
            album: "Some Wrong Album".into(), // deliberately wrong to defeat exact get
            duration: 134.0,
            translate: false,
            pronounce: false,
        };
        let lines = fetch_lyrics(&req).expect("ok").expect("found lyrics");
        eprintln!("lines={} synced={}", lines.len(), is_synced(&lines));
        let first = lines.iter().find(|l| !l.text.is_empty()).unwrap();
        eprintln!("[{:.2}] {}", first.time, first.text);
        assert!(lines.len() > 3);
        assert!(is_synced(&lines), "expected synced via search/netease fallback");
    }

    #[test]
    #[ignore = "hits the network"]
    fn live_fetch_and_translate() {
        let req = Request {
            key: "k".into(),
            artist: "Hitsujibungaku".into(),
            title: "more than words".into(),
            album: String::new(),
            duration: 289.0,
            translate: false,
            pronounce: false,
        };
        let lines = fetch_lyrics(&req).unwrap().unwrap();
        assert!(lines.len() > 5, "got {} lines", lines.len());
        assert!(lines.iter().any(|l| l.time > 0.0), "should be synced");
        let first_jp = lines.iter().find(|l| !l.text.is_empty()).unwrap().text.clone();
        let (tr, rm) = fetch_line(&first_jp, true, true);
        eprintln!("line:        {first_jp}");
        eprintln!("romaji(net): {}", rm.unwrap_or_default());
        eprintln!("translation: {}", tr.unwrap_or_default());
    }

    #[test]
    #[ignore = "hits the network"]
    fn live_pronunciation_any_language() {
        // Kanji reading, Korean, Russian — all via dt=rm.
        for (label, text) in [
            ("ja", "彼が言った言葉"),
            ("ko", "사랑해"),
            ("ru", "привет"),
        ] {
            let (_t, rm) = fetch_line(text, false, true);
            eprintln!("{label}: {text} -> {}", rm.clone().unwrap_or_default());
            assert!(rm.is_some(), "no romanization for {label}");
        }
    }
}

fn parse_lrc(s: &str) -> Vec<LyricLine> {
    let mut out: Vec<LyricLine> = Vec::new();
    for line in s.lines() {
        let mut rest = line;
        let mut times = Vec::new();
        while rest.starts_with('[') {
            let Some(end) = rest.find(']') else { break };
            let tag = &rest[1..end];
            match parse_time(tag) {
                Some(t) => {
                    times.push(t);
                    rest = &rest[end + 1..];
                }
                None => break, // metadata tag like [ar:...]
            }
        }
        let text = rest.trim().to_string();
        for t in times {
            out.push(LyricLine {
                time: t,
                text: text.clone(),
                ..Default::default()
            });
        }
    }
    out.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    split_inline_translation(merge_bilingual(out))
}

/// Some user-uploaded LRCs bake a translation right into every line as a trailing
/// parenthetical, e.g. `Boku no ... (Before my will breaks down)`. That English
/// is a translation, not part of the song, so it shouldn't sit in the karaoke
/// line. If most lines follow the pattern, peel the parenthetical into the
/// `translation` field (so it only shows when translation is enabled).
fn split_inline_translation(lines: Vec<LyricLine>) -> Vec<LyricLine> {
    let nonempty = lines.iter().filter(|l| !l.text.trim().is_empty()).count();
    if nonempty < 4 {
        return lines;
    }
    let matches = lines
        .iter()
        .filter(|l| !l.text.trim().is_empty() && trailing_paren(&l.text).is_some())
        .count();
    // Require a clear majority so songs with the odd parenthetical ad-lib are
    // left untouched.
    if (matches as f64) < 0.6 * nonempty as f64 {
        return lines;
    }
    lines
        .into_iter()
        .map(|mut l| {
            if let Some((main, inside)) = trailing_paren(&l.text) {
                if l.translation.is_none() {
                    l.translation = Some(inside);
                }
                l.text = main;
            }
            l
        })
        .collect()
}

/// Split `text (parenthetical)` into (text, parenthetical) when both parts are
/// non-empty. Uses the last `(` so nested/earlier parentheses in the lyric stay.
fn trailing_paren(s: &str) -> Option<(String, String)> {
    let t = s.trim_end();
    if !t.ends_with(')') {
        return None;
    }
    let open = t.rfind('(')?;
    let inside = t[open + 1..t.len() - 1].trim().to_string();
    let main = t[..open].trim().to_string();
    if inside.is_empty() || main.is_empty() {
        return None;
    }
    Some((main, inside))
}

/// Fold bilingual lyrics that arrive as two lines on the *same* timestamp (a
/// common NetEase format: original then translation) into a single line whose
/// `translation` carries the second text. Without this the active-line/karaoke
/// logic sees doubled lines at one time and the highlight/scroll desync.
fn merge_bilingual(lines: Vec<LyricLine>) -> Vec<LyricLine> {
    let mut out: Vec<LyricLine> = Vec::with_capacity(lines.len());
    for l in lines {
        if let Some(prev) = out.last_mut() {
            // Only merge real, identically-timed lines (skip the -1 "unsynced"
            // sentinel so plain lyrics aren't all collapsed together).
            if l.time >= 0.0 && prev.time >= 0.0 && (l.time - prev.time).abs() < 0.05 {
                let t = l.text.trim();
                if !t.is_empty() && t != prev.text.trim() && prev.translation.is_none() {
                    prev.translation = Some(l.text);
                }
                continue;
            }
        }
        out.push(l);
    }
    out
}

fn parse_time(tag: &str) -> Option<f64> {
    let (m, rest) = tag.split_once(':')?;
    let mins: f64 = m.trim().parse().ok()?;
    let secs: f64 = rest.trim().parse().ok()?;
    if !(0.0..60.0).contains(&secs) && mins == 0.0 {
        return None;
    }
    Some(mins * 60.0 + secs)
}

/// Fetch English translation and/or source romanization for one line via the
/// Google endpoint. `dt=t` yields translation; `dt=rm` yields a romanization of
/// the source — which works for *any* script (kanji readings, Hangul, Cyrillic,
/// Arabic, Devanagari, …), so it's our general "pronunciation" source.
fn fetch_line(text: &str, want_trans: bool, want_rm: bool) -> (Option<String>, Option<String>) {
    let mut url = String::from(
        "https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl=en",
    );
    if want_trans {
        url.push_str("&dt=t");
    }
    if want_rm {
        url.push_str("&dt=rm");
    }
    url.push_str("&q=");
    url.push_str(&url_encode(text));

    let Ok(body) = http_get(&url) else {
        return (None, None);
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) else {
        return (None, None);
    };
    let Some(segs) = json.get(0).and_then(|v| v.as_array()) else {
        return (None, None);
    };

    // Each sentence segment is [translation, source, _, romanization, ...]. The
    // translation chunks have a string at [0]; the romanization chunk has [0]
    // null and the reading at [3].
    let mut trans = String::new();
    let mut rm = String::new();
    for seg in segs {
        if let Some(t) = seg.get(0).and_then(|v| v.as_str()) {
            trans.push_str(t);
        }
        if let Some(r) = seg.get(3).and_then(|v| v.as_str()) {
            rm.push_str(r);
        }
    }

    let t = (want_trans && !trans.trim().is_empty()).then(|| trans.trim().to_string());
    let r = (want_rm && !rm.trim().is_empty()).then(|| rm.trim().to_string());
    (t, r)
}

/// One shared agent for all lyric/translation requests. It keeps a connection
/// pool alive, so the many same-host translation calls reuse a warm TLS
/// connection instead of re-handshaking on every line — the dominant cost when
/// fetching translation/pronunciation. Timeouts keep a stalled host from hanging
/// the fetch.
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(6)))
            .timeout_global(Some(Duration::from_secs(12)))
            .build();
        ureq::Agent::new_with_config(config)
    })
}

fn http_get(url: &str) -> Result<String, String> {
    http_get_ref(url, None)
}

fn http_get_ref(url: &str, referer: Option<&str>) -> Result<String, String> {
    let mut req = agent().get(url).header("User-Agent", "Mozilla/5.0 beatscope");
    if let Some(r) = referer {
        req = req.header("Referer", r);
    }
    let resp = req.call().map_err(|e| e.to_string())?;
    let mut body = String::new();
    resp.into_body()
        .into_reader()
        .take(4 * 1024 * 1024)
        .read_to_string(&mut body)
        .map_err(|e| e.to_string())?;
    Ok(body)
}

/// Percent-encode a query-parameter value (RFC 3986 unreserved kept as-is).
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}
