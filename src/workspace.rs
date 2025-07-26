use crate::{
    config::Config,
    connection::{DbWorkerRequest, DbWorkerResponse, SafeStmt, start_db_worker},
    focus::Focus,
    results::{Results, ResultsTab, ResultsContent},
    texteditor::{Editor, AppState},
};
use std::{
    sync::{Arc, Mutex},
    sync::mpsc::{Receiver, Sender},
    time::{Duration, Instant},
    io,
};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent},
    execute,
};
#[cfg(target_os = "windows")]
use crossterm::event::KeyEventKind;
use ratatui::{
    backend::Backend,
    Terminal,
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
};

const MIN_ROWS: i16 = 3;

pub struct Workspace {
    pub editor: Editor,
    pub results: Results,
    pub focus: Focus,
    pub running: bool,
    pub run_started: Option<Instant>,
    pub run_duration: Option<Duration>,
    pub error: Option<String>,
    pub connected: bool,
    
    // Database communication
    db_req_tx: Sender<DbWorkerRequest>,
    db_resp_rx: Receiver<DbWorkerResponse>,
    current_stmt: Arc<Mutex<Option<SafeStmt>>>,
    
    // Layout
    split_offset: i16,
    min_split_offset: i16,
    max_split_offset: i16,
    results_hidden: bool,
    editor_hidden: bool,
}

impl Workspace {
    pub fn new(config: Config) -> Self {
        let (db_req_tx, db_resp_rx, current_stmt) = start_db_worker(config.connection_string);
        
        Self {
            editor: Editor::new(),
            results: Results::new(),
            focus: Focus::Editor,
            running: false,
            run_started: None,
            run_duration: None,
            error: None,
            connected: false,
            db_req_tx,
            db_resp_rx,
            current_stmt,
            split_offset: 0,
            min_split_offset: -20,
            max_split_offset: 20,
            results_hidden: false,
            editor_hidden: false,
        }
    }
    
    pub fn run<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> io::Result<()> {
        // Windows-specific: Disable buffer optimization to force full redraws
        #[cfg(target_os = "windows")]
        {
            terminal.autoresize()?;
        }
        
        // Set title
        execute!(io::stdout(), crossterm::terminal::SetTitle("Minimal Frost"))?;
        
        loop {
            // Check if editor wants to exit
            if let AppState::Exiting = self.editor.app_state {
                break;
            }
            
            // Poll for database responses
            self.poll_db_responses();
            
            // Draw UI
            terminal.draw(|f| self.draw(f))?;
            
            // Handle events
            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) => {
                        // On Windows, ignore key release events
                        #[cfg(target_os = "windows")]
                        {
                            if key.kind == KeyEventKind::Release {
                                continue;
                            }
                        }
                        
                        if self.handle_key(key, terminal)? {
                            break; // Exit
                        }
                    }
                    Event::Mouse(mouse) => {
                        self.handle_mouse(mouse);
                    }
                    Event::Resize(_, _) => {
                        #[cfg(target_os = "windows")]
                        terminal.autoresize()?;
                    }
                    _ => {}
                }
            }
            
            // Update running timer
            if self.running {
                if let Some(started) = self.run_started {
                    self.run_duration = Some(started.elapsed());
                }
            }
        }
        
        Ok(())
    }
    
    fn draw(&mut self, f: &mut Frame) {
        let size = f.area();
        
        // Calculate constraints based on split_offset
        let editor_percent = ((50 + self.split_offset) as u16).clamp(20, 80);
        let results_percent = 100 - editor_percent;
        
        let constraints = if self.results_hidden {
            vec![Constraint::Percentage(100)]
        } else if self.editor_hidden {
            vec![Constraint::Percentage(0), Constraint::Percentage(100)]
        } else {
            vec![
                Constraint::Percentage(editor_percent),
                Constraint::Percentage(results_percent),
            ]
        };
        
        // Split vertically: editor on top, results below
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(size);
        
        // Draw editor if not hidden
        if !self.editor_hidden && !chunks.is_empty() {
            self.draw_editor(f, chunks[0]);
        }
        
        // Draw results if not hidden
        if !self.results_hidden && chunks.len() > 1 {
            self.results.render(f, chunks[1], self.focus == Focus::Results);
        } else if !self.results_hidden && self.editor_hidden {
            self.results.render(f, chunks[0], self.focus == Focus::Results);
        }
    }
    
    fn draw_editor(&mut self, f: &mut Frame, area: Rect) {
        // For now, use the texteditor's draw_ui directly
        // This isn't perfect but will work
        
        // The texteditor expects to draw on the full frame
        // So we need to create a temporary solution
        // For now, just draw a border and show the content
        let block = Block::default()
            .borders(Borders::ALL)
            .title("SQL Editor")
            .border_style(if self.focus == Focus::Editor {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::Gray)
            });
        
        let inner = block.inner(area);
        f.render_widget(block, area);
        
        // Simple text display for now
        let content = self.editor.rope.to_string();
        let paragraph = Paragraph::new(content);
        f.render_widget(paragraph, inner);
        
        // Show cursor if editor is focused
        if self.focus == Focus::Editor && !self.editor_hidden {
            // Set a simple cursor position
            if let Some((line, col)) = self.editor.get_position() {
                // Very simplified - doesn't account for viewport offset
                let cursor_x = inner.x + col.min(inner.width as usize - 1) as u16;
                let cursor_y = inner.y + line.min(inner.height as usize - 1) as u16;
                f.set_cursor_position((cursor_x, cursor_y));
            }
        }
    }
    
    fn handle_key<B: Backend>(&mut self, key: KeyEvent, terminal: &mut Terminal<B>) -> io::Result<bool> {
        // Global keys first
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
                // Set editor to exiting state
                self.editor.app_state = AppState::Exiting;
                return Ok(true);
            }
            (KeyCode::Tab, KeyModifiers::NONE) => {
                // Switch focus
                self.focus = match self.focus {
                    Focus::Editor => Focus::Results,
                    Focus::Results => Focus::Editor,
                    Focus::DbTree => Focus::Editor,
                };
                return Ok(false);
            }
            (KeyCode::Enter, KeyModifiers::CONTROL) => {
                self.run_query();
                return Ok(false);
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) if self.running => {
                self.cancel_query();
                return Ok(false);
            }
            // Alt+Arrow keys for resizing
            (KeyCode::Up, KeyModifiers::ALT) => {
                if !self.results_hidden {
                    self.split_offset = (self.split_offset + 5).min(self.max_split_offset);
                }
                return Ok(false);
            }
            (KeyCode::Down, KeyModifiers::ALT) => {
                if !self.results_hidden {
                    self.split_offset = (self.split_offset - 5).max(self.min_split_offset);
                }
                return Ok(false);
            }
            (KeyCode::Left, KeyModifiers::ALT) => {
                // Hide results (show editor only)
                self.results_hidden = true;
                self.editor_hidden = false;
                self.focus = Focus::Editor;
                return Ok(false);
            }
            (KeyCode::Right, KeyModifiers::ALT) => {
                // Hide editor (show results only)
                self.results_hidden = false;
                self.editor_hidden = true;
                self.focus = Focus::Results;
                return Ok(false);
            }
            (KeyCode::Char(' '), KeyModifiers::ALT) => {
                // Show both panes
                self.results_hidden = false;
                self.editor_hidden = false;
                return Ok(false);
            }
            _ => {}
        }
        
        // Route to focused pane
        match self.focus {
            Focus::Editor => {
                // Use the texteditor's key handling through our simplified interface
                // Get the terminal size for viewport calculations
                let size = terminal.size()?;
                let viewport_height = size.height.saturating_sub(2) as usize; // Account for borders
                let viewport_width = size.width.saturating_sub(2) as usize;
                
                // Call handle_editor_key directly
                crate::texteditor::handle_editor_key(&mut self.editor, key, viewport_width, viewport_height)?;
            }
            Focus::Results => {
                self.results.handle_key(key);
            }
            Focus::DbTree => {} // Not implemented yet
        }
        
        Ok(false)
    }
    
    fn handle_mouse(&mut self, _mouse: MouseEvent) {
        // TODO: Implement mouse handling for pane selection
    }
    
    fn poll_db_responses(&mut self) {
        while let Ok(response) = self.db_resp_rx.try_recv() {
            match response {
                DbWorkerResponse::Connected => {
                    self.connected = true;
                }
                DbWorkerResponse::QueryStarted { query_idx: _, started, query_context } => {
                    self.running = true;
                    self.run_started = Some(started);
                    // Add pending tab
                    let tab = ResultsTab::new_pending_with_start(query_context, started);
                    self.results.tabs.push(tab);
                    self.results.tab_idx = self.results.tabs.len() - 1;
                }
                DbWorkerResponse::QueryFinished { query_idx: _, elapsed: _, result } => {
                    self.running = false;
                    self.results.add_result(result);
                    self.focus = Focus::Results;
                }
                DbWorkerResponse::QueryError { query_idx: _, elapsed, message } => {
                    self.running = false;
                    self.run_duration = Some(elapsed);
                    self.error = Some(message.clone());
                    self.results.add_result(ResultsContent::Error {
                        message,
                        cursor: 0,
                        selection: None,
                    });
                }
            }
        }
    }
    
    fn run_query(&mut self) {
        if self.running || !self.connected {
            return;
        }
        
        let query = self.get_current_query();
        if query.is_empty() {
            return;
        }
        
        // Wrap in EXECUTE IMMEDIATE
        let wrapped_query = format!("EXECUTE IMMEDIATE $$\n{}\n$$", query);
        
        let _ = self.db_req_tx.send(DbWorkerRequest::RunQueries(vec![(wrapped_query, String::new())]));
    }
    
    fn cancel_query(&mut self) {
        if self.running {
            let _ = self.db_req_tx.send(DbWorkerRequest::Cancel);
        }
    }
    
    fn get_current_query(&self) -> String {
        // Get selected text or entire content from editor
        if self.editor.has_selection() {
            if let Some((start, end)) = self.editor.get_selection_range() {
                self.editor.rope.byte_slice(start..end).to_string()
            } else {
                String::new()
            }
        } else {
            self.editor.rope.to_string()
        }
    }
}