//! beatscope — a terminal music visualizer with album art (Kitty graphics
//! protocol) and transport controls beside a configurable cava-style spectrum.

mod app;
mod art;
mod audio;
mod cache;
mod config;
mod dsp;
mod lyrics;
mod lyrics_ui;
mod player;
mod romaji;
mod theme;
mod ui;
mod update;
mod visualizer;

use std::io;

use anyhow::Result;
use clap::Parser;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::app::App;
use crate::config::{Cli, Config};

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.update {
        return update::run(cli.force);
    }

    if cli.list_sources {
        println!("Available capture sources:");
        for s in audio::list_sources() {
            let tag = if s.ends_with(".monitor") {
                "  (monitor — plays-along audio)"
            } else {
                ""
            };
            println!("  {s}{tag}");
        }
        if let Some(def) = audio::default_monitor_source() {
            println!("\nDefault (auto): {def}");
        }
        return Ok(());
    }

    let mut cfg = Config::load();
    cfg.apply_cli(&cli);

    // Build the app first: it queries the terminal for graphics support before
    // we switch into raw mode / the alternate screen.
    let mut app = App::new(cfg)?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = app.run(&mut terminal);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}
