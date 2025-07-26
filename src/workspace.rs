use crate::{
    config::Config,
    connection::{DbWorkerRequest, DbWorkerResponse, SafeStmt, start_db_worker},
    focus::Focus,
    results::{Results, ResultsTab, ResultsContent},
    texteditor::Editor,
};
use std::{
    sync::{Arc, Mutex},
    sync::mpsc::{Receiver, Sender},
    time::{Duration, Instant},
    io,
};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent},
    cursor::SetCursorStyle,
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
    widgets::{Block, Borders},
};
use anyhow::Result;

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
    split_ratio: u16, // Percentage for editor (0-100)
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
            split_ratio: 60, // 60% editor, 40% results
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
                        
                        if self.handle_key(key)? {
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
        
        // Split horizontally: editor | results
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(self.split_ratio),
                Constraint::Percentage(100 - self.split_ratio),
            ])
            .split(size);
        
        // Draw editor in left pane
        self.draw_editor(f, chunks[0]);
        
        // Draw results in right pane
        self.results.render(f, chunks[1], self.focus == Focus::Results);
    }
    
    fn draw_editor(&mut self, f: &mut Frame, area: Rect) {
        // Use texteditor's draw_ui function but in our allocated area
        // We need to wrap it in a border to match our UI style
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
        
        // Now draw the editor content inside
        // For now, just show the text content
        use ratatui::widgets::Paragraph;
        let content = self.editor.rope.to_string();
        let paragraph = Paragraph::new(content);
        f.render_widget(paragraph, inner);
    }
    
    fn handle_key(&mut self, key: KeyEvent) -> io::Result<bool> {
        // Global keys
        match key.code {
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(true); // Exit
            }
            KeyCode::Tab if key.modifiers.is_empty() => {
                // Switch focus
                self.focus = match self.focus {
                    Focus::Editor => Focus::Results,
                    Focus::Results => Focus::Editor,
                    Focus::DbTree => Focus::Editor,
                };
                return Ok(false);
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.run_query();
                return Ok(false);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) && self.running => {
                self.cancel_query();
                return Ok(false);
            }
            _ => {}
        }
        
        // Route to focused pane
        match self.focus {
            Focus::Editor => {
                // Get viewport height for editor
                let viewport_height = 20; // TODO: Calculate actual height
                self.editor.handle_key_event(key, viewport_height)?;
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