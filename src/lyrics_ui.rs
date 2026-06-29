//! Renders the synced-lyrics panel: centered karaoke scroll, active line and
//! active-word highlighting, with optional rōmaji and translation sublines.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::lyrics::{Lyrics, Status};
use crate::theme::Palette;

pub struct LyricsView {
    pub palette: Palette,
    pub show_romaji: bool,
    pub show_translation: bool,
    /// Manual sync offset in seconds, for the title readout.
    pub offset: f64,
}

pub fn render(f: &mut Frame, area: Rect, lyrics: &Lyrics, pos: f64, view: &LyricsView) {
    let mut title = match lyrics.status {
        Status::Loaded => " lyrics ".to_string(),
        Status::Unsynced => " lyrics (unsynced) ".to_string(),
        Status::Loading => " lyrics · loading… ".to_string(),
        _ => " lyrics ".to_string(),
    };
    // Surface the active sync offset.
    if matches!(lyrics.status, Status::Loaded) && view.offset.abs() >= 0.05 {
        title = format!(" lyrics · sync {:+.2}s ", view.offset);
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(view.palette.accent_dim()))
        .title(Span::styled(
            title,
            Style::default()
                .fg(view.palette.accent())
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    match lyrics.status {
        Status::Idle | Status::Loading => return center_msg(f, inner, "loading lyrics…"),
        Status::NotFound => return center_msg(f, inner, "no lyrics found for this track"),
        Status::Error => return center_msg(f, inner, "lyrics unavailable"),
        _ => {}
    }

    let active = lyrics.active_index(pos);
    let active_word = active.and_then(|i| lyrics.active_word(i, pos));

    let dim = Style::default().fg(Color::Rgb(120, 120, 135));
    let sub = Style::default().fg(Color::Rgb(110, 130, 150));
    let trans_style = Style::default().fg(Color::Rgb(150, 150, 110));
    let hi = Style::default()
        .fg(view.palette.color(0.85))
        .add_modifier(Modifier::BOLD);
    let word_hi = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line> = Vec::new();
    let mut active_row = 0usize;

    for (i, l) in lyrics.lines.iter().enumerate() {
        let is_active = active == Some(i);
        if is_active {
            active_row = lines.len();
        }

        // Main text — split into words for active-word highlighting.
        if is_active && active_word.is_some() && l.text.split_whitespace().count() > 1 {
            let wi = active_word.unwrap();
            let mut spans = Vec::new();
            for (j, w) in l.text.split_whitespace().enumerate() {
                if j > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.push(Span::styled(w.to_string(), if j == wi { word_hi } else { hi }));
            }
            lines.push(Line::from(spans));
        } else {
            let text = if l.text.is_empty() { "♪" } else { &l.text };
            lines.push(Line::from(Span::styled(
                text.to_string(),
                if is_active { hi } else { dim },
            )));
        }

        if view.show_romaji {
            if let Some(r) = &l.romaji {
                lines.push(Line::from(Span::styled(
                    r.clone(),
                    sub.add_modifier(Modifier::ITALIC),
                )));
            }
        }
        if view.show_translation {
            if let Some(t) = &l.translation {
                lines.push(Line::from(Span::styled(t.clone(), trans_style)));
            }
        }
    }

    // Scroll so the active line sits at the vertical centre.
    let half = inner.height / 2;
    let offset = active_row.saturating_sub(half as usize) as u16;
    let para = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .scroll((offset, 0));
    f.render_widget(para, inner);
}

fn center_msg(f: &mut Frame, area: Rect, msg: &str) {
    let y = area.y + area.height / 2;
    let r = Rect::new(area.x, y, area.width, 1);
    f.render_widget(
        Paragraph::new(msg)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Rgb(120, 120, 135))),
        r,
    );
}
