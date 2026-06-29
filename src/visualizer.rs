//! The visualizer panel: several render modes sharing one band/sample feed.

use std::collections::VecDeque;

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::widgets::canvas::{Canvas, Context, Line as CanvasLine, Points};
use ratatui::widgets::{Block, Borders, Widget};
use ratatui::Frame;
use serde::{Deserialize, Serialize};

use crate::theme::Palette;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum VisMode {
    Bars,
    Peaks,
    Mirror,
    Wave,
    Circle,
    Spectrum,
    Bricks,
    Lanes,
    Rings,
    Spectro,
}

impl VisMode {
    /// All modes, in cycle/picker order.
    pub const ALL: [VisMode; 10] = [
        VisMode::Bars,
        VisMode::Peaks,
        VisMode::Mirror,
        VisMode::Spectrum,
        VisMode::Bricks,
        VisMode::Lanes,
        VisMode::Wave,
        VisMode::Circle,
        VisMode::Rings,
        VisMode::Spectro,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&m| m == self).unwrap_or(0)
    }

    /// One-line description for the picker.
    pub fn blurb(self) -> &'static str {
        use VisMode::*;
        match self {
            Bars => "classic vertical bars",
            Peaks => "bars with falling peak caps",
            Mirror => "bars mirrored about the centre",
            Wave => "filled oscilloscope",
            Circle => "radial spectrum ring",
            Spectrum => "continuous frequency fill",
            Bricks => "segmented LED meter",
            Lanes => "horizontal bars from the centre",
            Rings => "concentric tunnel rings",
            Spectro => "scrolling waterfall heatmap",
        }
    }

    pub fn name(self) -> &'static str {
        use VisMode::*;
        match self {
            Bars => "bars",
            Peaks => "peaks",
            Mirror => "mirror",
            Wave => "wave",
            Circle => "circle",
            Spectrum => "spectrum",
            Bricks => "bricks",
            Lanes => "lanes",
            Rings => "rings",
            Spectro => "spectro",
        }
    }

    /// How many frequency bands this mode wants given the inner panel width.
    pub fn desired_bars(self, inner_w: u16, bar_width: u16, bar_gap: u16) -> usize {
        match self {
            VisMode::Circle => 120,
            VisMode::Rings => 72,
            VisMode::Lanes => 64,
            VisMode::Spectro => 128,
            VisMode::Wave => 64, // unused by wave, but keep a sane value
            VisMode::Spectrum => (inner_w as usize / 2).clamp(48, 160),
            VisMode::Bars | VisMode::Peaks | VisMode::Mirror | VisMode::Bricks => {
                let step = (bar_width + bar_gap).max(1);
                ((inner_w / step) as usize).max(1)
            }
        }
    }
}

const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub struct VizParams<'a> {
    pub mode: VisMode,
    pub palette: Palette,
    pub bar_width: u16,
    pub bar_gap: u16,
    pub levels: &'a [f32],
    /// Pre-sampled, temporally smoothed waveform (-1..1 per column), wave mode only.
    pub wave: &'a [f32],
    /// Peak-hold values per band (peaks mode only).
    pub peaks: &'a [f32],
    /// Seconds since startup — drives the circle's rotation.
    pub time: f64,
    /// Bass-energy envelope (0..1) — drives the circle's pulsing ring.
    pub beat: f32,
    /// Per-frame spectrum history (oldest front, newest back) for `spectro` mode.
    pub spectro: &'a VecDeque<Vec<f32>>,
    pub title: String,
}

/// Symmetric layout: the lowest frequencies sit in the centre and the highest
/// at the two edges. Maps a horizontal position (0..1 across the panel) to a
/// fractional band index into a low→high band array.
fn sym_index(fpos: f32, nbands: usize) -> f32 {
    let edge = (fpos * 2.0 - 1.0).abs(); // 0 at the centre, 1 at the edges
    edge * nbands.saturating_sub(1) as f32
}

/// Linear sample of a band array at a fractional index.
fn sample_at(arr: &[f32], fi: f32) -> f32 {
    if arr.is_empty() {
        return 0.0;
    }
    let last = arr.len() - 1;
    let i0 = (fi.floor().max(0.0) as usize).min(last);
    let i1 = (i0 + 1).min(last);
    let fr = (fi - i0 as f32).clamp(0.0, 1.0);
    arr[i0] * (1.0 - fr) + arr[i1] * fr
}

pub fn render(f: &mut Frame, area: Rect, p: &VizParams) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(p.palette.accent_dim()))
        .title(ratatui::text::Span::styled(
            format!(" {} ", p.title),
            Style::default()
                .fg(p.palette.accent())
                .add_modifier(ratatui::style::Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    match p.mode {
        VisMode::Bars => draw_bars(f, inner, p, false),
        VisMode::Peaks => draw_peaks(f, inner, p),
        VisMode::Mirror => draw_bars(f, inner, p, true),
        VisMode::Spectrum => draw_spectrum(f, inner, p),
        VisMode::Wave => draw_wave(f, inner, p),
        VisMode::Circle => draw_circle(f, inner, p),
        VisMode::Bricks => draw_bricks(f, inner, p),
        VisMode::Lanes => draw_lanes(f, inner, p),
        VisMode::Rings => draw_rings(f, inner, p),
        VisMode::Spectro => draw_spectro(f, inner, p),
    }
}

/// Classic vertical bars (optionally mirrored about the vertical center).
fn draw_bars(f: &mut Frame, area: Rect, p: &VizParams, mirror: bool) {
    let buf = f.buffer_mut();
    let step = (p.bar_width + p.bar_gap).max(1);
    let slots = (area.width / step).max(1);
    let levels = p.levels;
    if levels.is_empty() {
        return;
    }

    for slot in 0..slots {
        let fpos = (slot as f32 + 0.5) / slots as f32;
        let value = sample_at(levels, sym_index(fpos, levels.len())).clamp(0.0, 1.0);
        let x0 = area.x + slot * step;

        for bw in 0..p.bar_width {
            let x = x0 + bw;
            if x >= area.x + area.width {
                break;
            }
            if mirror {
                draw_column_mirrored(buf, x, area, value, p.palette);
            } else {
                draw_column(buf, x, area, value, p.palette);
            }
        }
    }
}

/// Bars with a bright peak-hold cap that floats above each column and falls.
fn draw_peaks(f: &mut Frame, area: Rect, p: &VizParams) {
    let buf = f.buffer_mut();
    let step = (p.bar_width + p.bar_gap).max(1);
    let slots = (area.width / step).max(1);
    let levels = p.levels;
    if levels.is_empty() {
        return;
    }
    let h = area.height as f32;
    let bottom = area.y + area.height - 1;

    for slot in 0..slots {
        let fpos = (slot as f32 + 0.5) / slots as f32;
        let fi = sym_index(fpos, levels.len());
        let value = sample_at(levels, fi).clamp(0.0, 1.0);
        let peak = if p.peaks.is_empty() {
            value
        } else {
            sample_at(p.peaks, fi).clamp(0.0, 1.0).max(value)
        };
        let x0 = area.x + slot * step;

        for bw in 0..p.bar_width {
            let x = x0 + bw;
            if x >= area.x + area.width {
                break;
            }
            draw_column(buf, x, area, value, p.palette);
            // Peak cap: a bright marker at the held peak row.
            let prow = (peak * h).floor().min(h - 1.0).max(0.0) as u16;
            if prow > (value * h) as u16 {
                let y = bottom - prow;
                let cap = scale_rgb(p.palette.color(peak), 1.35);
                set(buf, x, y, '▀', cap);
            }
        }
    }
}

fn draw_column(buf: &mut ratatui::buffer::Buffer, x: u16, area: Rect, value: f32, pal: Palette) {
    let h = area.height as f32;
    let filled = value * h;
    let full = filled.floor() as u16;
    let frac = filled - full as f32;
    let bottom = area.y + area.height - 1;

    for row in 0..area.height {
        let y = bottom - row;
        let t = (row as f32 + 0.5) / h;
        if row < full {
            set(buf, x, y, '█', pal.color(t));
        } else if row == full && frac > 0.05 {
            let level = (frac * 8.0).round().clamp(1.0, 8.0) as usize;
            set(buf, x, y, BLOCKS[level], pal.color(t));
        }
    }
}

fn draw_column_mirrored(
    buf: &mut ratatui::buffer::Buffer,
    x: u16,
    area: Rect,
    value: f32,
    pal: Palette,
) {
    let h = area.height;
    let half = h / 2;
    if half == 0 {
        return;
    }
    // First row below / above the horizontal centre line.
    let lower0 = area.y + h / 2;
    let fill = (value.clamp(0.0, 1.0) * half as f32).round() as u16;

    for k in 0..fill {
        let t = k as f32 / half as f32;
        let color = pal.color(t);
        let yl = lower0 + k;
        if yl < area.y + h {
            set(buf, x, yl, '█', color);
        }
        if lower0 >= k + 1 {
            let yu = lower0 - 1 - k;
            if yu >= area.y {
                set(buf, x, yu, '█', color);
            }
        }
    }
}

fn set(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, ch: char, color: Color) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(ch);
        cell.set_fg(color);
    }
}

fn scale_rgb(c: Color, mul: f32) -> Color {
    if let Color::Rgb(r, g, b) = c {
        let f = |v: u8| (v as f32 * mul).clamp(0.0, 255.0) as u8;
        Color::Rgb(f(r), f(g), f(b))
    } else {
        c
    }
}

/// Continuous, full-width spectrum: every column filled, coloured by frequency
/// (horizontal gradient) with a bright leading edge. Distinct from the chunky,
/// gapped, height-coloured `bars` mode.
fn draw_spectrum(f: &mut Frame, area: Rect, p: &VizParams) {
    let buf = f.buffer_mut();
    let levels = p.levels;
    if levels.is_empty() {
        return;
    }
    let h = area.height as f32;
    let bottom = area.y + area.height - 1;
    let lastl = levels.len() - 1;

    for cx in 0..area.width {
        let fpos = (cx as f32 + 0.5) / area.width.max(1) as f32;
        // Symmetric (cava-style) layout: lows at the edges, highs in the centre.
        let fi = sym_index(fpos, levels.len());
        let value = sample_at(levels, fi).clamp(0.0, 1.0);

        // Colour by frequency band, so the gradient is symmetric about the centre.
        let base = p.palette.color(fi / lastl.max(1) as f32);
        let body = scale_rgb(base, 0.6); // dim, flat fill below the edge
        let edge = scale_rgb(base, 1.3); // brighter, same hue (no white)
        let filled = value * h;
        let full = filled.floor() as u16;
        let fr = filled - full as f32;
        let x = area.x + cx;

        for row in 0..full {
            let y = bottom - row;
            // Brighter leading edge only on tall enough columns; short columns
            // stay dim so the high-frequency end doesn't flash bright caps.
            let color = if row + 1 == full && full >= 2 { edge } else { body };
            set(buf, x, y, '█', color);
        }
        if full < area.height && fr > 0.05 {
            let y = bottom - full;
            let ch = BLOCKS[(fr * 8.0).round().clamp(1.0, 8.0) as usize];
            set(buf, x, y, ch, body);
        }
    }
}

/// Sample the raw audio into one value per column (-1..1), anchored on a rising
/// edge so the trace doesn't scroll/jitter. The app smooths this across frames
/// before it reaches [`draw_wave`].
pub fn sample_wave(samples: &[f32], width: usize) -> Vec<f32> {
    let width = width.max(1);
    let n = samples.len();
    if n < 4 {
        return vec![0.0; width];
    }
    let want = (width * 6).clamp(128, n);
    let base = n - want;
    let search = base.min(want);
    let eps = 0.012f32;
    // Hysteresis trigger: require a dip below -eps then a rise above +eps.
    let mut start = base;
    let mut armed = false;
    for i in (base - search)..base {
        if samples[i] < -eps {
            armed = true;
        } else if armed && samples[i] >= eps {
            start = i;
            break;
        }
    }
    let step = (want / width).max(1);
    let gain = 1.7;
    let mut out = Vec::with_capacity(width);
    for col in 0..width {
        let si = start + col * (want - 1) / (width.max(2) - 1);
        // Average the samples spanned by this column (low-pass) for a smooth line.
        let a = si.saturating_sub(step / 2);
        let b = (si + step / 2 + 1).min(n);
        let mut sum = 0.0;
        for &v in &samples[a..b] {
            sum += v;
        }
        out.push((sum / (b - a) as f32 * gain).clamp(-1.0, 1.0));
    }
    out
}

/// Oscilloscope: a filled braille waveform (area between the centre line and the
/// curve), giving it body without the chunkiness of full block cells.
fn draw_wave(f: &mut Frame, area: Rect, p: &VizParams) {
    let wave = p.wave;
    if wave.len() < 2 {
        return;
    }
    let pal = p.palette;
    let last = (wave.len() - 1) as f64;
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([0.0, last])
        .y_bounds([-1.1, 1.1])
        .paint(move |ctx: &mut Context| {
            // Fill from the centre to the curve for a substantial waveform body.
            for (i, &v) in wave.iter().enumerate() {
                let color = pal.color(v.abs().min(1.0));
                ctx.draw(&CanvasLine {
                    x1: i as f64,
                    y1: 0.0,
                    x2: i as f64,
                    y2: v as f64,
                    color,
                });
            }
            // Crisp connecting edge along the top of the curve.
            for i in 1..wave.len() {
                ctx.draw(&CanvasLine {
                    x1: (i - 1) as f64,
                    y1: wave[i - 1] as f64,
                    x2: i as f64,
                    y2: wave[i] as f64,
                    color: pal.color(wave[i].abs().min(1.0)),
                });
            }
        });
    canvas.render(area, f.buffer_mut());
}

/// Segmented LED meter: vertical bars broken into short lit blocks with gaps,
/// like a classic graphic equaliser.
fn draw_bricks(f: &mut Frame, area: Rect, p: &VizParams) {
    let buf = f.buffer_mut();
    let step = (p.bar_width + p.bar_gap).max(1);
    let slots = (area.width / step).max(1);
    let levels = p.levels;
    if levels.is_empty() {
        return;
    }
    let h = area.height as f32;
    let bottom = area.y + area.height - 1;

    for slot in 0..slots {
        let fpos = (slot as f32 + 0.5) / slots as f32;
        let value = sample_at(levels, sym_index(fpos, levels.len())).clamp(0.0, 1.0);
        let lit = (value * h).round() as u16;
        let x0 = area.x + slot * step;

        for bw in 0..p.bar_width {
            let x = x0 + bw;
            if x >= area.x + area.width {
                break;
            }
            for row in 0..area.height {
                // Two lit rows then a one-row gap gives the segmented LED look;
                // the topmost lit segment is brightened.
                if row < lit && row % 3 != 2 {
                    let y = bottom - row;
                    let t = (row as f32 + 0.5) / h;
                    let c = p.palette.color(t);
                    let c = if lit.saturating_sub(row) <= 1 {
                        scale_rgb(c, 1.4)
                    } else {
                        c
                    };
                    set(buf, x, y, '█', c);
                }
            }
        }
    }
}

/// Horizontal bars, one band per row, growing left and right from a central
/// vertical axis.
fn draw_lanes(f: &mut Frame, area: Rect, p: &VizParams) {
    let buf = f.buffer_mut();
    let levels = p.levels;
    if levels.is_empty() || area.width < 2 {
        return;
    }
    let half = area.width / 2;
    let cx = area.x + half;

    for row in 0..area.height {
        let y = area.y + row;
        // Top = high frequency, bottom = low.
        let ffrac = 1.0 - (row as f32 + 0.5) / area.height as f32;
        let fi = ffrac * levels.len().saturating_sub(1) as f32;
        let value = sample_at(levels, fi).clamp(0.0, 1.0);
        let len = (value * half as f32).round() as u16;

        for k in 0..len {
            let t = k as f32 / half.max(1) as f32;
            let color = p.palette.color((value * 0.5 + t * 0.5).min(1.0));
            let xr = cx + k;
            if xr < area.x + area.width {
                set(buf, xr, y, '█', color);
            }
            if cx >= area.x + 1 + k {
                set(buf, cx - 1 - k, y, '█', color);
            }
        }
    }
}

/// Terminal cells are roughly twice as tall as wide.
const CELL_ASPECT: f64 = 0.5;

/// Concentric rings whose levels flow outward over time, giving a tunnel effect.
fn draw_rings(f: &mut Frame, area: Rect, p: &VizParams) {
    let levels: Vec<f32> = p.levels.to_vec();
    let pal = p.palette;
    let time = p.time;
    let aspect = ((area.width as f64 * CELL_ASPECT) / area.height.max(1) as f64).max(0.05);
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([-aspect, aspect])
        .y_bounds([-1.0, 1.0])
        .paint(move |ctx: &mut Context| {
            use std::f64::consts::TAU;
            let n = levels.len();
            if n == 0 {
                return;
            }
            let rings = 26usize;
            let maxr = 0.97;
            for r in 0..rings {
                let depth = r as f64 / rings as f64;
                // Scroll which band maps to this radius so energy flows outward.
                let phase = (depth + time * 0.12).fract();
                let band = ((phase * n as f64) as usize).min(n - 1);
                let lv = levels[band] as f64;
                if lv < 0.05 {
                    continue;
                }
                let radius = depth * maxr;
                let color = scale_rgb(pal.color(lv as f32), (0.4 + lv) as f32);
                // More points on the outer (longer) rings so they stay solid.
                let steps = (60.0 + radius * 180.0) as usize;
                let pts: Vec<(f64, f64)> = (0..steps)
                    .map(|i| {
                        let a = i as f64 / steps as f64 * TAU;
                        (a.cos() * radius, a.sin() * radius)
                    })
                    .collect();
                ctx.draw(&Points {
                    coords: &pts,
                    color,
                });
            }
        });
    canvas.render(area, f.buffer_mut());
}

/// Scrolling spectrogram: time on the x-axis (newest at the right), frequency on
/// the y-axis (low at the bottom), intensity mapped to colour.
fn draw_spectro(f: &mut Frame, area: Rect, p: &VizParams) {
    let buf = f.buffer_mut();
    let hist = p.spectro;
    if hist.is_empty() {
        return;
    }
    let n = hist.len() as isize;
    let w = area.width as isize;
    let h = area.height as f32;

    for cx in 0..area.width {
        // Right-align the newest frame; blank any columns with no history yet.
        let fi = n - w + cx as isize;
        if fi < 0 {
            continue;
        }
        let frame = &hist[fi as usize];
        if frame.is_empty() {
            continue;
        }
        let x = area.x + cx;
        for ry in 0..area.height {
            let ffrac = 1.0 - (ry as f32 + 0.5) / h; // bottom = low freq
            let value = sample_at(frame, ffrac * frame.len().saturating_sub(1) as f32);
            if value < 0.04 {
                continue;
            }
            let color = scale_rgb(p.palette.color(value), (0.35 + value).min(1.4));
            set(buf, x, area.y + ry, '█', color);
        }
    }
}

/// Radial spectrum: bands wrapped around a circle, radius driven by level. The
/// whole wheel rotates slowly, and a ring inside it pulses to the beat.
fn draw_circle(f: &mut Frame, area: Rect, p: &VizParams) {
    let levels: Vec<f32> = p.levels.to_vec();
    let pal = p.palette;
    let beat = (p.beat as f64).clamp(0.0, 1.0);
    // Steady rotation (~one revolution every ~40s). The beat drives only the
    // inner ring below, not the rotation, so the wheel glides smoothly instead
    // of twitching.
    let rot = p.time * 0.16;
    // Widen the x data-range to match the area's true pixel aspect so the circle
    // stays round instead of stretching when the panel isn't square (e.g. split).
    let aspect = ((area.width as f64 * CELL_ASPECT) / area.height.max(1) as f64).max(0.05);
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([-aspect, aspect])
        .y_bounds([-1.0, 1.0])
        .paint(move |ctx: &mut Context| {
            use std::f64::consts::TAU;
            let n = levels.len();
            if n == 0 {
                return;
            }
            let inner = 0.34;

            // Pulsing inner ring: its radius swells and it brightens with the
            // bass envelope, so it visibly throbs on every kick.
            let pulse_r = inner * (0.72 + beat * 0.46);
            let ring_color = scale_rgb(pal.accent(), (0.45 + beat * 1.1) as f32);
            // A couple of concentric point-rings give the pulse some thickness.
            for &rad in &[pulse_r, pulse_r * 0.9] {
                let pts: Vec<(f64, f64)> = (0..220)
                    .map(|i| {
                        let a = i as f64 / 220.0 * TAU;
                        (a.cos() * rad, a.sin() * rad)
                    })
                    .collect();
                ctx.draw(&Points {
                    coords: &pts,
                    color: ring_color,
                });
            }

            let step = TAU / n as f64;
            for (i, &lv) in levels.iter().enumerate() {
                let a = i as f64 * step + rot;
                let outer = inner + (lv as f64) * 0.60;
                let color = pal.color(lv);
                // Draw a small fan per band so wedges read as solid spokes
                // rather than single braille hairlines.
                for k in 0..3 {
                    let aa = a + (k as f64 - 1.0) * step * 0.3;
                    let (ca, sa) = (aa.cos(), aa.sin());
                    ctx.draw(&CanvasLine {
                        x1: ca * inner,
                        y1: sa * inner,
                        x2: ca * outer,
                        y2: sa * outer,
                        color,
                    });
                }
            }
        });
    canvas.render(area, f.buffer_mut());
}
