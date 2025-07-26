mod config;
mod tile_rowstore;
mod workspace;
mod texteditor;
mod results;
mod connection;
mod focus;

use std::io;
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    Terminal,
};

fn main() -> Result<()> {
    // Load configuration
    let config = config::Config::load()?;
    
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    
    // Create workspace that wraps texteditor
    let mut workspace = workspace::Workspace::new(config);
    let res = workspace.run(&mut terminal);
    
    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    
    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
    }
    
    Ok(())
}