use crate::tile_rowstore::TileRowStore;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum ResultsContent {
    Table {
        headers: Vec<String>,
        tile_store: TileRowStore,
    },
    Error {
        message: String,
        cursor: usize,
        selection: Option<(usize, usize)>,
    },
    Info {
        message: String,
    },
    Pending,
}

pub struct ResultsTab {
    pub content: ResultsContent,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub view_row: usize,
    pub view_col: usize,
    pub running: bool,
    pub elapsed: Option<Duration>,
    pub run_started: Option<Instant>,
    pub query_context: String,
}

impl ResultsTab {
    pub fn new_pending(query_context: String) -> Self {
        Self::new_pending_with_start(query_context, Instant::now())
    }
    
    pub fn new_pending_with_start(query_context: String, started: Instant) -> Self {
        Self {
            content: ResultsContent::Pending,
            cursor_row: 0,
            cursor_col: 1,
            view_row: 0,
            view_col: 0,
            running: true,
            elapsed: None,
            run_started: Some(started),
            query_context,
        }
    }
}

pub struct Results {
    pub tabs: Vec<ResultsTab>,
    pub tab_idx: usize,
}

impl Results {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            tab_idx: 0,
        }
    }
    
    pub fn add_result(&mut self, result: ResultsContent) {
        // Find the pending tab and update it
        for tab in &mut self.tabs {
            if matches!(tab.content, ResultsContent::Pending) {
                tab.content = result;
                tab.running = false;
                tab.elapsed = tab.run_started.map(|s| s.elapsed());
                return;
            }
        }
        
        // If no pending tab, create a new one
        let mut tab = ResultsTab::new_pending(String::new());
        tab.content = result;
        tab.running = false;
        self.tabs.push(tab);
        self.tab_idx = self.tabs.len() - 1;
    }
    
    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab => {
                if !self.tabs.is_empty() && self.tabs.len() > 1 {
                    self.tab_idx = (self.tab_idx + 1) % self.tabs.len();
                }
            }
            _ => {}
        }
    }
    
    pub fn render(&self, frame: &mut Frame, area: Rect, focused: bool) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!("Results {}", 
                if self.tabs.is_empty() { 
                    String::new() 
                } else { 
                    format!("({}/{})", self.tab_idx + 1, self.tabs.len()) 
                }
            ))
            .border_style(if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::Gray)
            });
        
        let inner = block.inner(area);
        frame.render_widget(block, area);
        
        if self.tabs.is_empty() {
            let paragraph = Paragraph::new("No results yet. Press Ctrl+Enter to run a query.");
            frame.render_widget(paragraph, inner);
        } else if let Some(tab) = self.tabs.get(self.tab_idx) {
            match &tab.content {
                ResultsContent::Pending => {
                    let msg = if tab.running {
                        format!("Running query... ({:.1}s)", 
                            tab.run_started.map(|s| s.elapsed().as_secs_f32()).unwrap_or(0.0))
                    } else {
                        "Query pending...".to_string()
                    };
                    let paragraph = Paragraph::new(msg);
                    frame.render_widget(paragraph, inner);
                }
                ResultsContent::Info { message } => {
                    let paragraph = Paragraph::new(message.as_str());
                    frame.render_widget(paragraph, inner);
                }
                ResultsContent::Error { message, .. } => {
                    let paragraph = Paragraph::new(message.as_str())
                        .style(Style::default().fg(Color::Red));
                    frame.render_widget(paragraph, inner);
                }
                ResultsContent::Table { .. } => {
                    // TODO: Render actual table results
                    let paragraph = Paragraph::new("Table results will be displayed here");
                    frame.render_widget(paragraph, inner);
                }
            }
        }
    }
}