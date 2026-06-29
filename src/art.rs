//! Asynchronous album-art loading. Decoding (and any HTTP fetch) happens off the
//! render thread; decoded images are sent back for the Kitty protocol encoder.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use image::DynamicImage;

pub struct ArtLoader {
    tx: Sender<String>,
    rx: Receiver<DynamicImage>,
}

impl ArtLoader {
    pub fn start() -> ArtLoader {
        let (req_tx, req_rx) = mpsc::channel::<String>();
        let (img_tx, img_rx) = mpsc::channel::<DynamicImage>();
        thread::spawn(move || {
            const MAX_ATTEMPTS: u32 = 6;
            const RETRY: Duration = Duration::from_secs(3);
            let mut url: Option<String> = None;
            let mut attempts = 0;
            let mut done = false;

            loop {
                // Block until there's work, but only briefly while a retry is pending.
                let wait = if url.is_some() && !done && attempts < MAX_ATTEMPTS {
                    RETRY
                } else {
                    Duration::from_secs(3600)
                };
                match req_rx.recv_timeout(wait) {
                    Ok(mut u) => {
                        while let Ok(newer) = req_rx.try_recv() {
                            u = newer;
                        }
                        url = Some(u);
                        attempts = 0;
                        done = false;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }

                if let Some(u) = &url {
                    if !done && attempts < MAX_ATTEMPTS {
                        attempts += 1;
                        if let Some(img) = load(u) {
                            let _ = img_tx.send(img);
                            done = true;
                        }
                        // On failure we loop and retry after RETRY via recv_timeout.
                    }
                }
            }
        });
        ArtLoader {
            tx: req_tx,
            rx: img_rx,
        }
    }

    pub fn request(&self, url: String) {
        let _ = self.tx.send(url);
    }

    /// Non-blocking: returns a freshly decoded image if one is ready.
    pub fn poll(&self) -> Option<DynamicImage> {
        self.rx.try_recv().ok()
    }
}

fn load(url: &str) -> Option<DynamicImage> {
    let bytes = if let Some(rest) = url.strip_prefix("data:") {
        // data:[<mediatype>][;base64],<data>
        let comma = rest.find(',')?;
        let meta = &rest[..comma];
        let data = &rest[comma + 1..];
        if meta.contains("base64") {
            base64_decode(data)?
        } else {
            percent_decode(data).into_bytes()
        }
    } else if let Some(path) = url.strip_prefix("file://") {
        let decoded = percent_decode(path);
        std::fs::read(decoded).ok()?
    } else if url.starts_with("http://") || url.starts_with("https://") {
        let mut body = Vec::new();
        let resp = ureq::get(url).call().ok()?;
        use std::io::Read;
        resp.into_body()
            .into_reader()
            .take(16 * 1024 * 1024)
            .read_to_end(&mut body)
            .ok()?;
        body
    } else if url.starts_with('/') {
        std::fs::read(url).ok()?
    } else {
        return None;
    };
    // Reject truncated/empty payloads early. Firefox rotates and rewrites its
    // art cache files, so we can catch a file mid-write — a partial read decodes
    // to a tiny or broken image. Require a plausible minimum size so the retry
    // loop waits for the file to settle instead of showing a corrupt thumbnail.
    if bytes.len() < 256 {
        return None;
    }
    let img = image::load_from_memory(&bytes).ok()?;
    if img.width() < 16 || img.height() < 16 {
        return None;
    }
    Some(img)
}

/// Minimal standard-alphabet base64 decoder (handles whitespace and padding).
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = val(c)?;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Minimal percent-decoding for file:// paths (handles %20 etc.).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
