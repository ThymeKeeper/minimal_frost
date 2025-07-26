use crate::{
    config::Config,
    connection::{DbWorkerRequest, DbWorkerResponse, SafeStmt, start_db_worker},
    editor::Editor,
    focus::Focus,
    results::Results,
};
use std::{
    sync::{Arc, Mutex},
    sync::mpsc::{Receiver, Sender},
    time::{Duration, Instant},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
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
    split_offset: i16,
    min_split_offset: i16,
    max_split_offset: i16,
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
        }
    }
    
    pub fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        // Check for quit
        if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(true);
        }
        
        // Handle focus switching
        if key.code == KeyCode::Tab && key.modifiers.is_empty() {
            self.focus = match self.focus {
                Focus::Editor => Focus::Results,
                Focus::Results => Focus::Editor,
                Focus::DbTree => Focus::Editor,
            };
            return Ok(false);
        }
        
        // Handle running queries
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.run_query();
            return Ok(false);
        }
        
        // Cancel running queries
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) && self.running {
            self.cancel_query();
            return Ok(false);
        }
        
        // Route to focused pane
        match self.focus {
            Focus::Editor => self.editor.handle_key(key),
            Focus::Results => self.results.handle_key(key),
            Focus::DbTree => {} // Not implemented yet
        }
        
        Ok(false)
    }
    
    pub fn handle_mouse(&mut self, _mouse: MouseEvent) {
        // TODO: Implement mouse handling
    }
    
    pub fn poll_db_responses(&mut self) -> bool {
        let mut changed = false;
        
        while let Ok(response) = self.db_resp_rx.try_recv() {
            match response {
                DbWorkerResponse::Connected => {
                    self.connected = true;
                    changed = true;
                }
                DbWorkerResponse::QueryStarted { query_idx, started, query_context } => {
                    self.running = true;
                    self.run_started = Some(started);
                    changed = true;
                }
                DbWorkerResponse::QueryFinished { query_idx, elapsed, result } => {
                    self.running = false;
                    self.run_duration = Some(elapsed);
                    self.results.add_result(result);
                    self.focus = Focus::Results;
                    changed = true;
                }
                DbWorkerResponse::QueryError { query_idx, elapsed, message } => {
                    self.running = false;
                    self.run_duration = Some(elapsed);
                    self.error = Some(message);
                    changed = true;
                }
            }
        }
        
        changed
    }
    
    pub fn update(&mut self) {
        // Update running timer
        if self.running {
            if let Some(started) = self.run_started {
                self.run_duration = Some(started.elapsed());
            }
        }
    }
    
    pub fn render(&mut self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Percentage(50),
            ])
            .split(frame.area());
        
        // Render editor
        self.editor.render(frame, chunks[0], self.focus == Focus::Editor);
        
        // Render results
        self.results.render(frame, chunks[1], self.focus == Focus::Results);
        
        // Render status line
        self.render_status(frame);
    }
    
    fn render_status(&self, frame: &mut Frame) {
        // TODO: Implement status line
    }
    
    fn run_query(&mut self) {
        if self.running || !self.connected {
            return;
        }
        
        let query = self.editor.get_current_query();
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
}