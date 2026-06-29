//! The song panel: large album art (Kitty graphics protocol) on top, then
//! mouse-driven transport buttons, a draggable progress bar, volume, and the
//! live visualizer settings.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{FilterType, Resize, StatefulImage};

use crate::config::Config;
use crate::player::{NowPlaying, Status};

const DIM: Color = Color::Rgb(155, 155, 170);
const MUTED: Color = Color::Rgb(95, 95, 110);
const TRACK: Color = Color::Rgb(55, 55, 70);

/// Clickable regions returned to the app for mouse hit-testing.
#[derive(Clone, Copy, Default)]
pub struct SongHits {
    pub prev: Rect,
    pub play_pause: Rect,
    pub next: Rect,
    pub progress: Rect,
    pub volume: Rect,
}

pub fn render_song_panel(
    f: &mut Frame,
    area: Rect,
    np: &NowPlaying,
    cfg: &Config,
    protocol: &mut Option<StatefulProtocol>,
    drag_frac: Option<f64>,
    elapsed: f64,
) -> SongHits {
    let mut hits = SongHits::default();

    let accent = cfg.palette.accent();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(cfg.palette.accent_dim()))
        .title(Span::styled(
            " ♪ now playing ",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 4 || inner.height < 6 {
        return hits;
    }

    let art = art_rect(area);
    let rows = Layout::vertical([
        Constraint::Length(art.height),
        Constraint::Min(0),
    ])
    .split(inner);

    let status = if protocol.is_some() {
        ""
    } else if np.art_url.is_some() {
        "loading album art…"
    } else {
        "no album art"
    };
    render_art(f, rows[0], protocol, status);
    render_details(f, rows[1], np, cfg, drag_frac, elapsed, accent, &mut hits);
    hits
}

/// The album-art cell rectangle within a given song-panel area.
pub fn art_rect(panel: Rect) -> Rect {
    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(panel);
    if inner.width < 4 || inner.height < 6 {
        return Rect::new(inner.x, inner.y, 0, 0);
    }
    // Reserve space for the detail/controls block; give the rest to a big,
    // roughly-square album art (terminal cells are ~2:1, so height ≈ width/2).
    const DETAILS_H: u16 = 15;
    let art_h = (inner.width / 2)
        .min(inner.height.saturating_sub(DETAILS_H))
        .max(4);
    Rect::new(inner.x, inner.y, inner.width, art_h)
}

fn render_art(
    f: &mut Frame,
    area: Rect,
    protocol: &mut Option<StatefulProtocol>,
    status: &str,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    // Album art keeps its native pixels and nearest-scales to fill.
    let resize = Resize::Scale(Some(FilterType::Nearest));
    match protocol {
        Some(proto) => {
            f.render_stateful_widget(StatefulImage::default().resize(resize), area, proto);
        }
        None => {
            let placeholder = Paragraph::new(format!("\n\n♪\n\n{status}"))
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::default().fg(Color::Rgb(50, 50, 64))),
                );
            f.render_widget(placeholder, area);
        }
    }
}

fn render_details(
    f: &mut Frame,
    area: Rect,
    np: &NowPlaying,
    cfg: &Config,
    drag_frac: Option<f64>,
    elapsed: f64,
    accent: Color,
    hits: &mut SongHits,
) {
    let v = Layout::vertical([
        Constraint::Length(3), // title / artist / album
        Constraint::Length(1), // progress bar
        Constraint::Length(1), // times
        Constraint::Length(3), // transport buttons
        Constraint::Length(1), // volume
        Constraint::Min(0),    // settings + status
    ])
    .split(area);

    render_info(f, v[0], np, elapsed, accent);

    // A reported length is only trustworthy if playback hasn't already passed it;
    // some players report a wrong/short length, in which case we show the elapsed
    // time ticking and an unknown total rather than a stuck, maxed-out bar.
    let length_known = np.length > 1.0 && np.position <= np.length + 1.0;

    // Progress (draggable). While dragging, show the drag position immediately.
    let frac = match drag_frac {
        Some(d) => d,
        None if length_known => (np.position / np.length).clamp(0.0, 1.0),
        None => 0.0,
    };
    // Only allow seeking when we know the real length (drag maps to length).
    hits.progress = if length_known { v[1] } else { Rect::default() };
    f.render_widget(seek_bar(frac, v[1].width as usize, accent), v[1]);

    let shown_pos = if let (Some(d), true) = (drag_frac, length_known) {
        d * np.length
    } else {
        np.position
    };
    let total = if length_known {
        fmt_time(np.length)
    } else {
        "--:--".to_string()
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(fmt_time(shown_pos), Style::default().fg(DIM)),
            Span::styled("  /  ", Style::default().fg(MUTED)),
            Span::styled(total, Style::default().fg(MUTED)),
        ])),
        v[2],
    );

    render_transport(f, v[3], np.status, accent, hits);

    hits.volume = v[4];
    f.render_widget(volume_line(np.volume, v[4].width as usize), v[4]);

    render_settings(f, v[5], np, cfg, accent);
}

fn render_info(f: &mut Frame, area: Rect, np: &NowPlaying, elapsed: f64, accent: Color) {
    let w = area.width as usize;
    let lines = vec![
        Line::from(Span::styled(
            marquee(&np.title, w, elapsed),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            marquee(&np.artist, w, elapsed),
            Style::default().fg(accent),
        )),
        Line::from(Span::styled(
            marquee(&np.album, w, elapsed),
            Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
        )),
    ];
    f.render_widget(Paragraph::new(lines), area);
}

fn render_transport(f: &mut Frame, area: Rect, status: Status, accent: Color, hits: &mut SongHits) {
    // Three equal buttons across the row.
    let cells = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .spacing(1)
    .split(area);

    hits.prev = cells[0];
    hits.play_pause = cells[1];
    hits.next = cells[2];

    // One consistent icon colour/weight; only the centre glyph changes with state.
    // Geometric block-family glyphs keep equal width and baseline across buttons.
    let pp_glyph = match status {
        Status::Playing => "▌▌",
        _ => "▶",
    };
    let active = status == Status::Playing;
    button(f, cells[0], "◀◀", false, accent);
    button(f, cells[1], pp_glyph, active, accent);
    button(f, cells[2], "▶▶", false, accent);
}

fn button(f: &mut Frame, area: Rect, glyph: &str, active: bool, accent: Color) {
    let (fg, border) = if active {
        (accent, accent)
    } else {
        (Color::Rgb(205, 215, 235), Color::Rgb(90, 95, 115))
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border));
    let para = Paragraph::new(glyph)
        .alignment(Alignment::Center)
        .style(Style::default().fg(fg).add_modifier(Modifier::BOLD))
        .block(block);
    f.render_widget(para, area);
}

fn render_settings(f: &mut Frame, area: Rect, np: &NowPlaying, cfg: &Config, accent: Color) {
    // A little gradient swatch makes the active palette unmistakable.
    let mut swatch: Vec<Span> = Vec::new();
    for k in 0..6 {
        let t = k as f32 / 5.0;
        swatch.push(Span::styled("█", Style::default().fg(cfg.palette.color(t))));
    }
    let mut palette_spans = vec![Span::styled("palette ", Style::default().fg(MUTED))];
    palette_spans.extend(swatch);
    palette_spans.push(Span::styled(format!(" {}", cfg.palette.name()), Style::default().fg(accent)));

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("mode ", Style::default().fg(MUTED)),
            Span::styled(format!("{:<9}", cfg.mode.name()), Style::default().fg(accent)),
        ]),
        Line::from(palette_spans),
        Line::from(vec![
            Span::styled("gain ", Style::default().fg(MUTED)),
            Span::styled(format!("{:<6.2}", cfg.gain), Style::default().fg(DIM)),
            Span::styled("agc ", Style::default().fg(MUTED)),
            Span::styled(
                if cfg.auto_gain { "on" } else { "off" },
                Style::default().fg(if cfg.auto_gain {
                    Color::Rgb(120, 230, 140)
                } else {
                    MUTED
                }),
            ),
        ]),
    ];

    if !np.connected {
        lines.push(Line::from(Span::styled(
            "no MPRIS player detected",
            Style::default().fg(Color::Rgb(220, 120, 120)),
        )));
    } else if !np.player_name.is_empty() {
        lines.push(Line::from(Span::styled(
            truncate(&format!("via {}", np.player_name), area.width as usize),
            Style::default().fg(MUTED),
        )));
    }

    // Pin compact hints to the bottom row.
    let body_h = area.height.saturating_sub(1);
    let split = Layout::vertical([Constraint::Length(body_h), Constraint::Length(1)]).split(area);
    f.render_widget(Paragraph::new(lines), split[0]);
    let hint = truncate(
        "drag bar to seek · m mode · c palette · ? help",
        area.width as usize,
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(hint, Style::default().fg(MUTED)))),
        split[1],
    );
}

fn seek_bar(frac: f64, width: usize, accent: Color) -> Paragraph<'static> {
    Paragraph::new(bar(frac, width, accent, '█', '░'))
}

fn volume_line(vol: f64, width: usize) -> Paragraph<'static> {
    let label = format!("vol {:>3.0}% ", vol * 100.0);
    let used = label.chars().count();
    let bw = width.saturating_sub(used).max(1);
    let mut spans = vec![Span::styled(label, Style::default().fg(MUTED))];
    spans.extend(bar(vol.clamp(0.0, 1.0), bw, Color::Rgb(160, 160, 200), '█', '░').spans);
    Paragraph::new(Line::from(spans))
}

fn bar(frac: f64, width: usize, color: Color, full: char, empty: char) -> Line<'static> {
    let filled = ((frac * width as f64).round() as usize).min(width);
    let mut spans = Vec::new();
    if filled > 0 {
        spans.push(Span::styled(
            full.to_string().repeat(filled),
            Style::default().fg(color),
        ));
    }
    if filled < width {
        spans.push(Span::styled(
            empty.to_string().repeat(width - filled),
            Style::default().fg(TRACK),
        ));
    }
    Line::from(spans)
}

fn fmt_time(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "0:00".into();
    }
    let s = secs as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

use unicode_width::UnicodeWidthChar;

fn display_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Truncate to a display-column budget (CJK-aware), adding an ellipsis.
fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if display_width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw > max.saturating_sub(1) {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

/// Horizontally scroll text that's wider than `width` (display columns), looping
/// with a separator. Returns a slice whose display width fits `width`.
fn marquee(s: &str, width: usize, elapsed: f64) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(s) <= width {
        return s.to_string();
    }
    let sep = "   ·   ";
    let full: Vec<char> = s.chars().chain(sep.chars()).collect();
    let len = full.len();
    let speed = 3.0; // chars per second
    let start = ((elapsed * speed) as usize) % len;

    let mut out = String::new();
    let mut used = 0usize;
    let mut i = 0;
    while used < width {
        let c = full[(start + i) % len];
        let cw = c.width().unwrap_or(0);
        if used + cw > width {
            out.push(' '); // pad a trailing wide-char gap
            used += 1;
            continue;
        }
        out.push(c);
        used += cw;
        i += 1;
        if i > len * 2 {
            break;
        }
    }
    out
}
