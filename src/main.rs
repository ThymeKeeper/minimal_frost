mod config;
mod tile_rowstore;
mod workspace;
mod editor;
mod results;
mod connection;
mod focus;

use std::io;
use std::time::{Duration, Instant};
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
    
    // Create app and run
    let mut workspace = workspace::Workspace::new(config);
    let res = run_app(&mut terminal, &mut workspace);
    
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

fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    workspace: &mut workspace::Workspace,
) -> Result<()> {
    let mut last_draw = Instant::now();
    let mut dirty = true;
    
    loop {
        // Handle database responses
        if workspace.poll_db_responses() {
            dirty = true;
        }
        
        // Poll for events with timeout
        let timeout = Duration::from_millis(50);
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    if workspace.handle_key(key)? {
                        break; // Exit on quit
                    }
                    dirty = true;
                }
                Event::Mouse(mouse) => {
                    workspace.handle_mouse(mouse);
                    dirty = true;
                }
                Event::Resize(_, _) => {
                    dirty = true;
                }
                _ => {}
            }
        }
        
        // Update workspace state
        workspace.update();
        
        // Render if needed (with frame rate limiting)
        if dirty && last_draw.elapsed() >= Duration::from_millis(16) {
            terminal.draw(|frame| workspace.render(frame))?;
            last_draw = Instant::now();
            dirty = false;
        }
    }
    
    Ok(())
}