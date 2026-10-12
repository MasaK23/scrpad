//! The notes and window preferences, saved as JSON in the user's config directory.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::theme::ThemeKind;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Note {
    /// Stable identity across tab moves (editor undo history and undo toasts key on it).
    #[serde(default)]
    pub id: u64,
    pub title: String,
    pub text: String,
    /// Masked on screen (API keys and the like); the text itself is unchanged.
    #[serde(default)]
    pub hidden: bool,
}

impl Note {
    pub fn new(title: String) -> Self {
        Self { id: 0, title, text: String::new(), hidden: false }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum FontStyle {
    #[default]
    Sans,
    Mono,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct State {
    pub notes: Vec<Note>,
    pub active: usize,
    pub pinned: bool,
    pub theme: ThemeKind,
    pub font: FontStyle,
    pub font_size: f32,
    /// Last floating window size and position in logical points (position is not restorable
    /// everywhere: Wayland lets only the compositor place windows).
    pub window_size: Option<[f32; 2]>,
    pub window_pos: Option<[f32; 2]>,
}

impl Default for State {
    fn default() -> Self {
        let mut state = Self {
            notes: vec![Note::new("Scratchpad 1".into())],
            active: 0,
            pinned: true,
            theme: ThemeKind::Dark,
            font: FontStyle::Sans,
            font_size: 14.5,
            window_size: None,
            window_pos: None,
        };
        state.fix_up();
        state
    }
}

impl State {
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("scratchpad").join("notes.json"))
    }

    /// A file that exists but can't be read is moved aside to `notes.json.bak` rather than
    /// being overwritten by the next autosave.
    pub fn load(path: &std::path::Path) -> Self {
        let mut state = match std::fs::read_to_string(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            read => match read.map_err(|e| e.to_string()).and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string())) {
                Ok(state) => state,
                Err(e) => {
                    let bak = path.with_extension("json.bak");
                    eprintln!("scratchpad: could not load {} ({e}); moved it to {}", path.display(), bak.display());
                    let _ = std::fs::rename(path, &bak);
                    State::default()
                }
            },
        };
        state.fix_up();
        state
    }

    /// Writes atomically (temp file + rename) so a crash never leaves half a file.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }

    pub fn fix_up(&mut self) {
        if self.notes.is_empty() {
            self.notes.push(Note::new("Scratchpad 1".into()));
        }
        self.active = self.active.min(self.notes.len() - 1);
        // Give every note a unique non-zero id (older files and new notes start at 0).
        let mut seen = std::collections::HashSet::new();
        for i in 0..self.notes.len() {
            if self.notes[i].id == 0 || !seen.insert(self.notes[i].id) {
                self.notes[i].id = self.fresh_id();
                seen.insert(self.notes[i].id);
            }
        }
        self.font_size = self.font_size.clamp(10.0, 32.0);
        // A size from a broken file or a huge tiled window must not come back as the float size.
        if self.window_size.is_some_and(|[w, h]| !(200.0..=4000.0).contains(&w) || !(150.0..=4000.0).contains(&h)) {
            self.window_size = None;
        }
    }

    pub fn fresh_id(&self) -> u64 {
        self.notes.iter().map(|n| n.id).max().unwrap_or(0) + 1
    }

    /// "Scratchpad N" with the smallest N not already used.
    pub fn next_title(&self) -> String {
        (1..)
            .map(|n| format!("Scratchpad {n}"))
            .find(|t| !self.notes.iter().any(|n| &n.title == t))
            .unwrap()
    }

    pub fn add_note(&mut self) {
        let mut note = Note::new(self.next_title());
        note.id = self.fresh_id();
        self.notes.push(note);
        self.active = self.notes.len() - 1;
    }

    pub fn close_note(&mut self, i: usize) {
        if i >= self.notes.len() {
            return;
        }
        self.notes.remove(i);
        if self.notes.is_empty() {
            self.notes.push(Note::new("Scratchpad 1".into()));
            self.fix_up();
        }
        if self.active > i || self.active >= self.notes.len() {
            self.active = self.active.saturating_sub(1);
        }
    }

    pub fn current(&self) -> &Note {
        &self.notes[self.active]
    }

    pub fn current_mut(&mut self) -> &mut Note {
        &mut self.notes[self.active]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_fill_gaps_and_closing_keeps_one() {
        let mut s = State::default();
        s.add_note();
        s.add_note();
        assert_eq!(s.notes.iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), ["Scratchpad 1", "Scratchpad 2", "Scratchpad 3"]);
        s.close_note(1);
        assert_eq!(s.active, 1);
        assert_eq!(s.next_title(), "Scratchpad 2");
        s.close_note(0);
        s.close_note(0);
        assert_eq!(s.notes.len(), 1);
        assert_eq!(s.active, 0);
    }

    #[test]
    fn ids_are_unique_and_corrupt_files_are_kept() {
        let mut s = State::default();
        s.add_note();
        s.add_note();
        let ids: std::collections::HashSet<_> = s.notes.iter().map(|n| n.id).collect();
        assert_eq!(ids.len(), 3);
        assert!(!ids.contains(&0));

        let dir = std::env::temp_dir().join(format!("scratchpad-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notes.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(State::load(&path).notes.len(), 1);
        assert_eq!(std::fs::read_to_string(dir.join("notes.json.bak")).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn round_trips_through_json() {
        let dir = std::env::temp_dir().join(format!("scratchpad-store-{}", std::process::id()));
        let path = dir.join("notes.json");
        let mut s = State::default();
        s.current_mut().text = "sk-secret".into();
        s.current_mut().hidden = true;
        s.save(&path).unwrap();
        assert_eq!(State::load(&path), s);
        let _ = std::fs::remove_dir_all(dir);
    }
}
