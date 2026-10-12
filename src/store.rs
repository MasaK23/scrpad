//! The notes and window preferences, saved as JSON in the user's config directory.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use zeroize::Zeroize;

use crate::crypto;
use crate::theme::ThemeKind;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Note {
    /// Stable identity across tab moves (editor undo history and undo toasts key on it).
    #[serde(default)]
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub text: String,
    /// Masked on screen (API keys and the like); the text itself is unchanged.
    #[serde(default)]
    pub hidden: bool,
    /// The text as a SimpleTextCrypt message. Set: the note is encrypted and `text` never reaches the disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cipher: Option<String>,
    /// Runtime only: the password of an unlocked encrypted note.
    #[serde(skip)]
    pub password: Option<String>,
}

/// What a note looks like on disk: the plaintext of an encrypted note is written as "".
#[derive(Serialize)]
struct NoteOnDisk<'a> {
    id: u64,
    title: &'a str,
    text: &'a str,
    hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cipher: Option<&'a str>,
}

impl Serialize for Note {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        NoteOnDisk {
            id: self.id,
            title: &self.title,
            text: if self.is_encrypted() { "" } else { &self.text },
            hidden: self.hidden,
            cipher: self.cipher.as_deref(),
        }
        .serialize(serializer)
    }
}

impl Note {
    pub fn new(title: String) -> Self {
        Self { id: 0, title, text: String::new(), hidden: false, cipher: None, password: None }
    }

    pub fn is_encrypted(&self) -> bool {
        self.cipher.is_some()
    }

    /// Encrypted and no password yet: `text` is empty.
    pub fn is_locked(&self) -> bool {
        self.is_encrypted() && self.password.is_none()
    }

    /// Encrypts the text with `password` (also changes the password of an encrypted note).
    pub fn encrypt(&mut self, password: &str) -> Result<(), crypto::Error> {
        self.cipher = Some(crypto::encrypt(password, &self.text)?);
        self.password = Some(password.to_owned());
        Ok(())
    }

    /// Re-encrypts the current text with the stored password; a no-op unless unlocked and encrypted.
    pub fn reseal(&mut self) -> Result<(), crypto::Error> {
        if let (Some(_), Some(password)) = (&self.cipher, &self.password) {
            self.cipher = Some(crypto::encrypt(password, &self.text)?);
        }
        Ok(())
    }

    /// Decrypts the cipher into `text` and keeps the password for `reseal`.
    pub fn unlock(&mut self, password: &str) -> Result<(), crypto::Error> {
        let cipher = self.cipher.as_deref().ok_or(crypto::Error::InvalidFormat)?;
        self.text = crypto::decrypt(password, cipher)?;
        self.password = Some(password.to_owned());
        Ok(())
    }

    /// Wipes the text and password from memory. `reseal` first: edits since the last one are lost.
    pub fn lock(&mut self) {
        if self.is_encrypted() {
            self.text.zeroize();
            self.password.zeroize();
            self.password = None;
        }
    }

    /// Drops the encryption and keeps the text; a no-op on a locked note (its text is not there).
    pub fn remove_encryption(&mut self) {
        if !self.is_locked() {
            self.cipher = None;
            self.password.zeroize();
            self.password = None;
        }
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
    /// Ids are never reused: editor state (undo history included) is keyed on them, and a new
    /// note must not inherit what a closed, possibly encrypted, note held.
    pub next_id: u64,
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
            next_id: 1,
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
        // An encrypted note without a password is locked: whatever text it came with goes.
        for note in self.notes.iter_mut().filter(|n| n.is_locked()) {
            note.text.clear();
        }
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

    pub fn fresh_id(&mut self) -> u64 {
        let id = self.next_id.max(self.notes.iter().map(|n| n.id).max().unwrap_or(0) + 1);
        self.next_id = id + 1;
        id
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

    #[test]
    fn encrypted_notes_never_write_plaintext() {
        let dir = std::env::temp_dir().join(format!("scratchpad-crypt-{}", std::process::id()));
        let path = dir.join("notes.json");
        let mut s = State::default();
        s.current_mut().text = "the launch codes".into();
        s.current_mut().encrypt("hunter2").unwrap();
        assert!(!s.current().is_locked());
        s.current_mut().text.push_str(" v2");
        s.current_mut().reseal().unwrap();
        s.save(&path).unwrap();
        let json = std::fs::read_to_string(&path).unwrap();
        assert!(!json.contains("launch codes") && !json.contains("hunter2"), "{json}");
        assert!(json.contains("\"cipher\""));

        let mut loaded = State::load(&path);
        let note = loaded.current_mut();
        assert!(note.is_locked() && note.text.is_empty());
        assert_eq!(note.unlock("wrong"), Err(crypto::Error::DecryptFailed));
        assert!(note.is_locked());
        note.unlock("hunter2").unwrap();
        assert_eq!(note.text, "the launch codes v2");
        note.lock();
        assert!(note.is_locked() && note.text.is_empty());
        note.remove_encryption();
        assert!(note.is_encrypted(), "a locked note keeps its cipher");
        note.unlock("hunter2").unwrap();
        note.remove_encryption();
        assert!(!note.is_encrypted() && note.password.is_none());
        assert_eq!(note.text, "the launch codes v2");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn old_files_load_and_stray_plaintext_is_dropped() {
        let s: State = serde_json::from_str(r#"{"notes":[{"title":"a","text":"plain"},{"title":"b","text":"leak","cipher":"x"}]}"#).unwrap();
        let mut s = s;
        s.fix_up();
        assert_eq!(s.notes[0].text, "plain");
        assert!(!s.notes[0].is_encrypted());
        assert!(s.notes[1].is_locked() && s.notes[1].text.is_empty());
    }
}
