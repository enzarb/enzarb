//! A minimal single-line text input.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextInput {
    pub value: String,
    /// Byte offset of the cursor.
    pub cursor: usize,
}

impl TextInput {
    pub fn new(s: impl Into<String>) -> Self {
        let value = s.into();
        Self {
            cursor: value.len(),
            value,
        }
    }

    pub fn set(&mut self, s: &str) {
        self.value = s.to_string();
        self.cursor = self.value.len();
    }

    /// Apply an editing key; returns whether the key was consumed.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('u') if ctrl => {
                self.value.clear();
                self.cursor = 0;
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                self.value.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace => {
                if let Some(c) = self.value[..self.cursor].chars().next_back() {
                    self.cursor -= c.len_utf8();
                    self.value.remove(self.cursor);
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.value.len() {
                    self.value.remove(self.cursor);
                }
            }
            KeyCode::Left => {
                if let Some(c) = self.value[..self.cursor].chars().next_back() {
                    self.cursor -= c.len_utf8();
                }
            }
            KeyCode::Right => {
                if let Some(c) = self.value[self.cursor..].chars().next() {
                    self.cursor += c.len_utf8();
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.value.len(),
            _ => return false,
        }
        true
    }

    pub fn trimmed(&self) -> &str {
        self.value.trim()
    }

    /// Characters before the cursor (for placing the terminal cursor).
    pub fn cursor_col(&self) -> usize {
        self.value[..self.cursor].chars().count()
    }
}
