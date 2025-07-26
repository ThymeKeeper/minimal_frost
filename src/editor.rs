use arboard::Clipboard;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use ropey::Rope;
use std::{
    path::PathBuf,
    time::Instant,
};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy)]
struct VisualLine {
    start_byte: usize,
    end_byte: usize,
    is_continuation: bool,
    indent: usize,
    logical_line: usize,
}

#[derive(Clone, Debug)]
enum EditOp {
    Insert { pos: usize, text: String },
    Delete { pos: usize, text: String },
}

struct UndoGroup {
    ops: Vec<(EditOp, usize, usize)>,
    timestamp: Instant,
}

pub struct Editor {
    rope: Rope,
    caret: usize,
    selection_anchor: Option<usize>,
    preferred_col: usize,
    viewport_offset: (usize, usize),
    word_wrap: bool,
    visual_lines: Vec<Option<VisualLine>>,
    visual_lines_valid: bool,
    logical_line_map: Vec<(usize, usize)>,
    scrolloff: usize,
    modified: bool,
    undo_stack: Vec<UndoGroup>,
    redo_stack: Vec<UndoGroup>,
    current_group: Option<UndoGroup>,
    last_edit_time: Option<Instant>,
    clipboard: Clipboard,
}

impl Editor {
    pub fn new() -> Self {
        let clipboard = Clipboard::new().unwrap_or_else(|_| {
            panic!("Failed to initialize clipboard");
        });
        
        Self {
            rope: Rope::new(),
            caret: 0,
            selection_anchor: None,
            preferred_col: 0,
            viewport_offset: (0, 0),
            word_wrap: true,
            visual_lines: Vec::new(),
            visual_lines_valid: false,
            logical_line_map: Vec::new(),
            scrolloff: 3,
            modified: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            current_group: None,
            last_edit_time: None,
            clipboard,
        }
    }
    
    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_char(ch);
            }
            KeyCode::Enter => {
                self.insert_char('\n');
            }
            KeyCode::Backspace => {
                self.delete_char_before();
            }
            KeyCode::Delete => {
                self.delete_char_after();
            }
            KeyCode::Left => {
                self.move_caret_left(key.modifiers.contains(KeyModifiers::SHIFT));
            }
            KeyCode::Right => {
                self.move_caret_right(key.modifiers.contains(KeyModifiers::SHIFT));
            }
            KeyCode::Up => {
                self.move_caret_up(key.modifiers.contains(KeyModifiers::SHIFT));
            }
            KeyCode::Down => {
                self.move_caret_down(key.modifiers.contains(KeyModifiers::SHIFT));
            }
            KeyCode::Home => {
                self.move_to_line_start(key.modifiers.contains(KeyModifiers::SHIFT));
            }
            KeyCode::End => {
                self.move_to_line_end(key.modifiers.contains(KeyModifiers::SHIFT));
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.select_all();
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.copy();
            }
            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cut();
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.paste();
            }
            KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.undo();
            }
            KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.redo();
            }
            _ => {}
        }
    }
    
    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        // Update visual lines if needed
        if !self.visual_lines_valid {
            self.update_visual_lines(area.width as usize);
        }
        
        // Create block
        let block = Block::default()
            .borders(Borders::ALL)
            .title("SQL Editor")
            .border_style(if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::Gray)
            });
        
        let inner = block.inner(area);
        frame.render_widget(block, area);
        
        // Render content
        let mut lines = Vec::new();
        let viewport_height = inner.height as usize;
        
        for row in 0..viewport_height {
            let visual_line_idx = self.viewport_offset.0 + row;
            if visual_line_idx >= self.visual_lines.len() {
                break;
            }
            
            if let Some(vline) = &self.visual_lines[visual_line_idx] {
                let text = self.rope.byte_slice(vline.start_byte..vline.end_byte).to_string();
                let line_style = Style::default();
                
                // Handle selection
                if let Some(anchor) = self.selection_anchor {
                    let (start, end) = if anchor < self.caret {
                        (anchor, self.caret)
                    } else {
                        (self.caret, anchor)
                    };
                    
                    if vline.start_byte < end && vline.end_byte > start {
                        // Line contains selection
                        let mut spans = Vec::new();
                        let mut current_pos = vline.start_byte;
                        
                        // Before selection
                        if current_pos < start && start < vline.end_byte {
                            let before_text = self.rope.byte_slice(current_pos..start).to_string();
                            spans.push(Span::styled(before_text, line_style));
                            current_pos = start;
                        }
                        
                        // Selection
                        let sel_end = end.min(vline.end_byte);
                        if current_pos < sel_end {
                            let sel_text = self.rope.byte_slice(current_pos..sel_end).to_string();
                            spans.push(Span::styled(
                                sel_text,
                                Style::default().bg(Color::Blue).fg(Color::White),
                            ));
                            current_pos = sel_end;
                        }
                        
                        // After selection
                        if current_pos < vline.end_byte {
                            let after_text = self.rope.byte_slice(current_pos..vline.end_byte).to_string();
                            spans.push(Span::styled(after_text, line_style));
                        }
                        
                        lines.push(Line::from(spans));
                    } else {
                        lines.push(Line::styled(text, line_style));
                    }
                } else {
                    lines.push(Line::styled(text, line_style));
                }
            } else {
                lines.push(Line::from(""));
            }
        }
        
        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, inner);
        
        // Render cursor if focused
        if focused {
            let caret_visual_pos = self.get_caret_visual_position();
            if let Some((row, col)) = caret_visual_pos {
                if row >= self.viewport_offset.0 && row < self.viewport_offset.0 + viewport_height {
                    let screen_row = row - self.viewport_offset.0;
                    let screen_col = col.saturating_sub(self.viewport_offset.1);
                    if screen_col < inner.width as usize {
                        frame.set_cursor_position((
                            inner.x + screen_col as u16,
                            inner.y + screen_row as u16,
                        ));
                    }
                }
            }
        }
    }
    
    pub fn get_current_query(&self) -> String {
        // Get the entire content or selected text
        if let Some(anchor) = self.selection_anchor {
            let (start, end) = if anchor < self.caret {
                (anchor, self.caret)
            } else {
                (self.caret, anchor)
            };
            self.rope.byte_slice(start..end).to_string()
        } else {
            self.rope.to_string()
        }
    }
    
    // Private helper methods
    fn insert_char(&mut self, ch: char) {
        self.delete_selection();
        let text = ch.to_string();
        self.add_edit_op(EditOp::Insert {
            pos: self.caret,
            text: text.clone(),
        });
        self.rope.insert(self.caret, &text);
        self.caret += text.len();
        self.visual_lines_valid = false;
        self.modified = true;
        self.selection_anchor = None;
    }
    
    fn delete_char_before(&mut self) {
        if self.selection_anchor.is_some() {
            self.delete_selection();
        } else if self.caret > 0 {
            let prev_char_start = self.prev_char_boundary(self.caret);
            let text = self.rope.byte_slice(prev_char_start..self.caret).to_string();
            self.add_edit_op(EditOp::Delete {
                pos: prev_char_start,
                text,
            });
            self.rope.remove(prev_char_start..self.caret);
            self.caret = prev_char_start;
            self.visual_lines_valid = false;
            self.modified = true;
        }
    }
    
    fn delete_char_after(&mut self) {
        if self.selection_anchor.is_some() {
            self.delete_selection();
        } else if self.caret < self.rope.len_bytes() {
            let next_char_end = self.next_char_boundary(self.caret);
            let text = self.rope.byte_slice(self.caret..next_char_end).to_string();
            self.add_edit_op(EditOp::Delete {
                pos: self.caret,
                text,
            });
            self.rope.remove(self.caret..next_char_end);
            self.visual_lines_valid = false;
            self.modified = true;
        }
    }
    
    fn delete_selection(&mut self) {
        if let Some(anchor) = self.selection_anchor {
            let (start, end) = if anchor < self.caret {
                (anchor, self.caret)
            } else {
                (self.caret, anchor)
            };
            
            let text = self.rope.byte_slice(start..end).to_string();
            self.add_edit_op(EditOp::Delete { pos: start, text });
            self.rope.remove(start..end);
            self.caret = start;
            self.selection_anchor = None;
            self.visual_lines_valid = false;
            self.modified = true;
        }
    }
    
    fn move_caret_left(&mut self, select: bool) {
        if !select && self.selection_anchor.is_some() {
            self.selection_anchor = None;
        } else if select && self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.caret);
        }
        
        if self.caret > 0 {
            self.caret = self.prev_char_boundary(self.caret);
            self.update_preferred_col();
        }
    }
    
    fn move_caret_right(&mut self, select: bool) {
        if !select && self.selection_anchor.is_some() {
            self.selection_anchor = None;
        } else if select && self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.caret);
        }
        
        if self.caret < self.rope.len_bytes() {
            self.caret = self.next_char_boundary(self.caret);
            self.update_preferred_col();
        }
    }
    
    fn move_caret_up(&mut self, select: bool) {
        // TODO: Implement proper visual line navigation
        if !select && self.selection_anchor.is_some() {
            self.selection_anchor = None;
        } else if select && self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.caret);
        }
    }
    
    fn move_caret_down(&mut self, select: bool) {
        // TODO: Implement proper visual line navigation
        if !select && self.selection_anchor.is_some() {
            self.selection_anchor = None;
        } else if select && self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.caret);
        }
    }
    
    fn move_to_line_start(&mut self, select: bool) {
        if !select && self.selection_anchor.is_some() {
            self.selection_anchor = None;
        } else if select && self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.caret);
        }
        
        let line = self.rope.byte_to_line(self.caret);
        self.caret = self.rope.line_to_byte(line);
        self.preferred_col = 0;
    }
    
    fn move_to_line_end(&mut self, select: bool) {
        if !select && self.selection_anchor.is_some() {
            self.selection_anchor = None;
        } else if select && self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.caret);
        }
        
        let line = self.rope.byte_to_line(self.caret);
        let line_start = self.rope.line_to_byte(line);
        let line_end = if line < self.rope.len_lines() - 1 {
            self.rope.line_to_byte(line + 1) - 1
        } else {
            self.rope.len_bytes()
        };
        self.caret = line_end;
        self.update_preferred_col();
    }
    
    fn select_all(&mut self) {
        self.selection_anchor = Some(0);
        self.caret = self.rope.len_bytes();
    }
    
    fn copy(&mut self) {
        if let Some(anchor) = self.selection_anchor {
            let (start, end) = if anchor < self.caret {
                (anchor, self.caret)
            } else {
                (self.caret, anchor)
            };
            let text = self.rope.byte_slice(start..end).to_string();
            let _ = self.clipboard.set_text(text);
        }
    }
    
    fn cut(&mut self) {
        if self.selection_anchor.is_some() {
            self.copy();
            self.delete_selection();
        }
    }
    
    fn paste(&mut self) {
        if let Ok(text) = self.clipboard.get_text() {
            self.delete_selection();
            self.add_edit_op(EditOp::Insert {
                pos: self.caret,
                text: text.clone(),
            });
            self.rope.insert(self.caret, &text);
            self.caret += text.len();
            self.visual_lines_valid = false;
            self.modified = true;
        }
    }
    
    fn undo(&mut self) {
        self.finalize_undo_group();
        if let Some(group) = self.undo_stack.pop() {
            for (op, _, _) in group.ops.iter().rev() {
                match op {
                    EditOp::Insert { pos, text } => {
                        self.rope.remove(*pos..*pos + text.len());
                        self.caret = *pos;
                    }
                    EditOp::Delete { pos, text } => {
                        self.rope.insert(*pos, text);
                        self.caret = *pos + text.len();
                    }
                }
            }
            self.redo_stack.push(group);
            self.visual_lines_valid = false;
            self.selection_anchor = None;
        }
    }
    
    fn redo(&mut self) {
        if let Some(group) = self.redo_stack.pop() {
            for (op, _, _) in &group.ops {
                match op {
                    EditOp::Insert { pos, text } => {
                        self.rope.insert(*pos, text);
                        self.caret = *pos + text.len();
                    }
                    EditOp::Delete { pos, text } => {
                        self.rope.remove(*pos..*pos + text.len());
                        self.caret = *pos;
                    }
                }
            }
            self.undo_stack.push(group);
            self.visual_lines_valid = false;
            self.selection_anchor = None;
        }
    }
    
    fn add_edit_op(&mut self, op: EditOp) {
        let now = Instant::now();
        let should_start_new_group = if let Some(last_time) = self.last_edit_time {
            now.duration_since(last_time).as_millis() > 1000
        } else {
            true
        };
        
        if should_start_new_group {
            self.finalize_undo_group();
            self.current_group = Some(UndoGroup {
                ops: vec![(op, self.caret, self.selection_anchor.unwrap_or(self.caret))],
                timestamp: now,
            });
        } else if let Some(ref mut group) = self.current_group {
            group.ops.push((op, self.caret, self.selection_anchor.unwrap_or(self.caret)));
        }
        
        self.last_edit_time = Some(now);
        self.redo_stack.clear();
    }
    
    fn finalize_undo_group(&mut self) {
        if let Some(group) = self.current_group.take() {
            if !group.ops.is_empty() {
                self.undo_stack.push(group);
            }
        }
    }
    
    fn update_visual_lines(&mut self, viewport_width: usize) {
        self.visual_lines.clear();
        self.logical_line_map.clear();
        
        // Simple line wrapping
        for line_idx in 0..self.rope.len_lines() {
            let line_start = self.rope.line_to_byte(line_idx);
            let line_end = if line_idx < self.rope.len_lines() - 1 {
                self.rope.line_to_byte(line_idx + 1) - 1
            } else {
                self.rope.len_bytes()
            };
            
            self.visual_lines.push(Some(VisualLine {
                start_byte: line_start,
                end_byte: line_end,
                is_continuation: false,
                indent: 0,
                logical_line: line_idx,
            }));
        }
        
        self.visual_lines_valid = true;
    }
    
    fn get_caret_visual_position(&self) -> Option<(usize, usize)> {
        for (row, vline) in self.visual_lines.iter().enumerate() {
            if let Some(vline) = vline {
                if self.caret >= vline.start_byte && self.caret <= vline.end_byte {
                    let text = self.rope.byte_slice(vline.start_byte..self.caret).to_string();
                    let col = text.width();
                    return Some((row, col));
                }
            }
        }
        None
    }
    
    fn update_preferred_col(&mut self) {
        if let Some((_, col)) = self.get_caret_visual_position() {
            self.preferred_col = col;
        }
    }
    
    fn prev_char_boundary(&self, pos: usize) -> usize {
        if pos == 0 {
            return 0;
        }
        
        // In ropey, we can use byte_to_char and char_to_byte to find boundaries
        let char_idx = self.rope.byte_to_char(pos.min(self.rope.len_bytes()));
        if char_idx > 0 {
            self.rope.char_to_byte(char_idx - 1)
        } else {
            0
        }
    }
    
    fn next_char_boundary(&self, pos: usize) -> usize {
        let len = self.rope.len_bytes();
        if pos >= len {
            return len;
        }
        
        // In ropey, we can use byte_to_char and char_to_byte to find boundaries
        let char_idx = self.rope.byte_to_char(pos);
        if char_idx < self.rope.len_chars() {
            self.rope.char_to_byte(char_idx + 1)
        } else {
            len
        }
    }
}