//! The whole window: header (title, pin, close), tab strip, editor and footer toolbar.
//! Everything is laid out in absolute rects inside the frameless window and painted by hand.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;

use blake2::{Blake2s256, Digest};
use egui::{Align2, Color32, CursorIcon, Id, Pos2, Rect, ResizeDirection, Response, Sense, Stroke, Vec2, ViewportCommand};
use zeroize::Zeroizing;

use crate::crypto;
use crate::icons;
use crate::store::{FontStyle, Note, State};
use crate::theme::{self, ThemeKind, Tokens};
use crate::update;

const PAD: f32 = 12.0;
const HEADER_H: f32 = 36.0;
const TABS_H: f32 = 36.0;
const FOOTER_H: f32 = 42.0;
const RESIZE_EDGE: f32 = 6.0;
const SAVE_DELAY: f64 = 0.4;
const SAVE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(3);
const TOAST_SECS: f64 = 4.0;
/// Long enough to read the version and reach the Download button.
const UPDATE_TOAST_SECS: f64 = 12.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Popup {
    Font,
    More,
    Encrypt,
}

/// What an Undo in the toast brings back.
enum Undo {
    /// `password` is set when the cleared note was encrypted: brought back, it stays encrypted.
    Cleared {
        id: u64,
        title: String,
        text: String,
        password: Option<String>,
    },
    Closed {
        index: usize,
        note: Note,
    },
}

struct Toast {
    text: String,
    until: f64,
    action: Option<ToastAction>,
}

/// The button a toast can carry.
enum ToastAction {
    Undo(Undo),
    /// Opens a release page in the browser.
    Open(String),
}

impl ToastAction {
    fn label(&self) -> &'static str {
        match self {
            ToastAction::Undo(_) => "Undo",
            ToastAction::Open(_) => "Download",
        }
    }
}

/// Keeps the pin in step with Hyprland, which (unlike winit on Wayland) can pin a window.
#[cfg(target_os = "linux")]
struct WmSync {
    hypr: crate::hyprland::Hyprland,
    /// Window state changes seen on the compositor side (SUPER+O, SUPER+T, moving, resizing).
    observed: mpsc::Receiver<crate::hyprland::WindowState>,
    /// The saved size and position still to be restored once the window floats at start-up.
    restore: bool,
    /// A pin state still to be sent, and until when to retry (the window may not be mapped yet).
    pending: Option<(bool, f64)>,
    /// Observations older than our own last change are stale until this time.
    ignore_until: f64,
}

pub struct App {
    pub state: State,
    path: Option<PathBuf>,
    dirty_since: Option<f64>,
    save_retry_at: Option<std::time::Instant>,
    save_error: Option<String>,
    save_blocked: bool,
    force_close: bool,
    fonts_ready: bool,
    applied_theme: Option<ThemeKind>,
    popup: Option<(Popup, Rect)>,
    /// Tab index, edited title, and whether the field still needs to grab focus.
    renaming: Option<(usize, String, bool)>,
    /// The active tab the strip last scrolled into view (scrolling only follows changes).
    scrolled_to: Option<u64>,
    toast: Option<Toast>,
    saves: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    /// A "Check for updates…" the user started, still waiting for GitHub.
    update_check: Option<mpsc::Receiver<update::Outcome>>,
    /// Asks the editor to take keyboard focus on the next frame (after switching or adding a tab).
    focus_editor: bool,
    encrypt_form: EncryptForm,
    /// The locked note the unlock field belongs to, what is typed in it, and an error to show.
    unlock_form: (u64, String, Option<&'static str>),
    /// Per encrypted note: a hash of the plaintext its `cipher` holds. Differs from the text: stale.
    sealed: HashMap<u64, [u8; 32]>,
    /// Encrypted notes being re-sealed on a worker thread (Argon2id takes a noticeable moment).
    sealing: HashSet<u64>,
    seal_jobs: mpsc::Sender<SealJob>,
    seal_rx: mpsc::Receiver<Sealed>,
    /// Where the footer's lock button was last drawn (the Encrypt popup anchors to it).
    lock_button: Rect,
    /// Bumped on every encrypt, lock, unlock and decrypt: sealing results from before are void.
    epoch: u64,
    /// Notes whose editor state (and with it the undo history of their plaintext) must go.
    forget: Vec<u64>,
    /// The encrypted note whose cipher is shown in place of its text (read-only).
    show_cipher: Option<u64>,
    #[cfg(target_os = "linux")]
    wm: Option<WmSync>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, state: State, path: Option<PathBuf>, startup_error: Option<String>) -> Self {
        #[allow(unused_mut)]
        let mut app = Self::with_state_inner(&cc.egui_ctx, state, path, startup_error);
        #[cfg(target_os = "linux")]
        if let Some(hypr) = crate::hyprland::Hyprland::detect() {
            let (tx, observed) = mpsc::channel();
            let (watcher, ctx) = (hypr.clone(), cc.egui_ctx.clone());
            std::thread::spawn(move || {
                let mut last = None;
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let Some(now) = watcher.window_state() else { continue };
                    if last != Some(now) {
                        last = Some(now);
                        if tx.send(now).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                }
            });
            app.wm = Some(WmSync { hypr, observed, pending: Some((app.state.pinned, f64::NAN)), restore: true, ignore_until: 0.0 });
        }
        app
    }

    /// `path: None` keeps everything in memory (tests).
    pub fn with_state(ctx: &egui::Context, state: State, path: Option<PathBuf>) -> Self {
        Self::with_state_inner(ctx, state, path, None)
    }

    fn with_state_inner(ctx: &egui::Context, mut state: State, path: Option<PathBuf>, startup_error: Option<String>) -> Self {
        state.fix_up();
        egui_extras::install_image_loaders(ctx);
        let (seal_jobs, seal_rx) = spawn_sealer();
        let sealed = state.notes.iter().filter(|n| n.is_encrypted() && !n.is_locked()).map(|n| (n.id, text_hash(&n.text))).collect();
        let save_blocked = startup_error.is_some();
        Self {
            state,
            path,
            dirty_since: None,
            save_retry_at: None,
            save_error: startup_error,
            save_blocked,
            force_close: false,
            fonts_ready: false,
            applied_theme: None,
            popup: None,
            renaming: None,
            scrolled_to: None,
            toast: None,
            saves: None,
            update_check: None,
            focus_editor: true,
            encrypt_form: Default::default(),
            unlock_form: Default::default(),
            sealed,
            sealing: HashSet::new(),
            seal_jobs,
            seal_rx,
            lock_button: Rect::NOTHING,
            epoch: 0,
            forget: Vec::new(),
            show_cipher: None,
            #[cfg(target_os = "linux")]
            wm: None,
        }
    }

    fn tokens(&self) -> Tokens {
        Tokens::of(self.state.theme)
    }

    fn mark_dirty(&mut self, now: f64) {
        self.dirty_since.get_or_insert(now);
    }

    /// Saves right away, first re-sealing every stale encrypted note on this thread (on quit).
    pub fn save_now(&mut self) -> bool {
        for i in 0..self.state.notes.len() {
            self.seal_sync(i);
        }
        if self.state.notes.iter().any(|note| self.is_stale(note)) {
            return false;
        }
        self.write()
    }

    /// The debounced autosave: stale encrypted notes are re-sealed on a worker and written
    /// once done; until then the file keeps their previous cipher, never their plaintext.
    fn autosave(&mut self, ctx: &egui::Context) {
        let mut worker_ok = true;
        for note in &self.state.notes {
            if self.is_stale(note) && self.sealing.insert(note.id) {
                let job = SealJob {
                    id: note.id,
                    epoch: self.epoch,
                    text: Zeroizing::new(note.text.clone()),
                    password: Zeroizing::new(note.password.clone().unwrap_or_default()),
                };
                if self.seal_jobs.send(job).is_err() {
                    self.sealing.remove(&note.id);
                    worker_ok = false;
                }
            }
        }
        if !self.sealing.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if worker_ok {
            if !self.write() {
                ctx.request_repaint();
                ctx.request_repaint_after(SAVE_RETRY_DELAY);
            }
        } else {
            self.dirty_since.get_or_insert(0.0);
            self.save_error = Some("Changes not saved: encryption worker stopped".into());
            ctx.request_repaint();
        }
    }

    fn write(&mut self) -> bool {
        if self.save_blocked {
            return false;
        }
        match self.path.as_ref().map_or(Ok(()), |path| self.state.save(path)) {
            Ok(()) => {
                // A toast (and its Undo) stays: it was shown beside the error and expires on its own.
                self.save_error = None;
                self.dirty_since = None;
                self.save_retry_at = None;
                true
            }
            Err(e) => {
                let path = self.path.as_deref().expect("a save error has a path");
                eprintln!("scrpad: could not save {}: {e}", path.display());
                self.dirty_since.get_or_insert(0.0);
                self.save_retry_at = Some(std::time::Instant::now() + SAVE_RETRY_DELAY);
                self.save_error = Some(format!("Changes not saved: {e}"));
                false
            }
        }
    }

    fn is_stale(&self, note: &Note) -> bool {
        note.is_encrypted() && !note.is_locked() && self.sealed.get(&note.id) != Some(&text_hash(&note.text))
    }

    /// Brings the cipher of note `i` up to date on this thread.
    fn seal_sync(&mut self, i: usize) {
        if !self.is_stale(&self.state.notes[i]) {
            return;
        }
        let note = &mut self.state.notes[i];
        match note.reseal() {
            Ok(()) => {
                self.sealed.insert(note.id, text_hash(&note.text));
            }
            Err(e) => {
                eprintln!("scrpad: could not encrypt “{}”: {e}", note.title);
                self.dirty_since.get_or_insert(0.0);
                self.save_error = Some(format!("Changes not saved: encryption failed for “{}”", note.title));
            }
        }
    }

    /// Takes in ciphers finished by the sealing workers.
    fn poll_sealed(&mut self, ctx: &egui::Context, now: f64) {
        while let Ok(done) = self.seal_rx.try_recv() {
            self.sealing.remove(&done.id);
            let Some(note) = self.state.notes.iter_mut().find(|n| n.id == done.id) else { continue };
            // Locked, unlocked, re-encrypted or decrypted meanwhile: that path already wrote what it
            // needed, and this cipher may hold an older text or password. Still stale: seal again.
            if done.epoch != self.epoch || note.is_locked() || !note.is_encrypted() {
                self.mark_dirty(now);
                continue;
            }
            match done.cipher {
                Ok(cipher) => {
                    note.cipher = Some(cipher);
                    self.sealed.insert(done.id, done.hash);
                    // Typed on meanwhile: the next autosave seals again.
                    self.mark_dirty(now);
                }
                Err(e) => {
                    eprintln!("scrpad: could not encrypt “{}”: {e}", note.title);
                    self.dirty_since.get_or_insert(now);
                    self.save_error = Some(format!("Changes not saved: encryption failed for “{}”", note.title));
                }
            }
        }
        if !self.sealing.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn toast(&mut self, now: f64, text: impl Into<String>, undo: Option<Undo>) {
        self.toast = Some(Toast { text: text.into(), until: now + TOAST_SECS, action: undo.map(ToastAction::Undo) });
    }

    /// Asks GitHub for the latest release on a worker thread. Only ever started by the user.
    pub fn check_for_updates(&mut self, now: f64) {
        if self.update_check.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.update_check = Some(rx);
        self.toast(now, "Checking for updates…", None);
        std::thread::spawn(move || {
            let _ = tx.send(update::check());
        });
    }

    fn poll_update_check(&mut self, ctx: &egui::Context, now: f64) {
        let Some(rx) = &self.update_check else { return };
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
                return;
            }
            Err(mpsc::TryRecvError::Disconnected) => update::Outcome::Failed("the check stopped".into()),
        };
        self.update_check = None;
        self.show_update_outcome(outcome, now);
    }

    fn show_update_outcome(&mut self, outcome: update::Outcome, now: f64) {
        match outcome {
            update::Outcome::UpToDate { current } => self.toast(now, format!("scrpad {current} is the latest version"), None),
            update::Outcome::Available { version, url } => {
                self.toast = Some(Toast {
                    text: format!("scrpad {version} is available"),
                    until: now + UPDATE_TOAST_SECS,
                    action: Some(ToastAction::Open(url)),
                });
            }
            update::Outcome::Failed(e) => {
                eprintln!("scrpad: update check failed: {e}");
                self.toast(now, "Could not check for updates", None);
            }
        }
    }

    // ---- actions (also used by keyboard shortcuts and tests) -------------------------------

    pub fn add_note(&mut self, now: f64) {
        self.state.add_note();
        self.focus_editor = true;
        self.mark_dirty(now);
    }

    pub fn close_note(&mut self, i: usize, now: f64) {
        let note = self.state.notes[i].clone();
        self.state.close_note(i);
        self.forget.push(note.id);
        if !note.text.is_empty() || note.is_encrypted() {
            let msg = format!("Closed “{}”", note.title);
            self.toast(now, msg, Some(Undo::Closed { index: i, note }));
        }
        self.renaming = None;
        self.mark_dirty(now);
    }

    pub fn toggle_hidden(&mut self, now: f64) {
        let note = self.state.current_mut();
        note.hidden = !note.hidden;
        self.mark_dirty(now);
    }

    pub fn toggle_pin(&mut self, ctx: &egui::Context, now: f64) {
        self.state.pinned = !self.state.pinned;
        self.mark_dirty(now);
        #[cfg(target_os = "linux")]
        if let Some(wm) = &mut self.wm {
            wm.pending = Some((self.state.pinned, now + 2.0));
            return;
        }
        let level = if self.state.pinned { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(level));
    }

    pub fn copy(&mut self, ctx: &egui::Context, now: f64) {
        if self.showing_cipher()
            && let Some(cipher) = self.state.current().cipher.clone()
        {
            ctx.copy_text(cipher);
            self.toast(now, "Copied the encrypted text", None);
            return;
        }
        if self.state.current().is_locked() {
            self.toast(now, "Unlock the note first", None);
            return;
        }
        let text = self.state.current().text.clone();
        if text.is_empty() {
            self.toast(now, "Nothing to copy", None);
            return;
        }
        ctx.copy_text(text);
        self.toast(now, "Copied to clipboard", None);
    }

    pub fn clear(&mut self, now: f64) {
        let note = self.state.current_mut();
        if note.is_locked() {
            return;
        }
        let (id, title, password) = (note.id, note.title.clone(), note.password.clone());
        let text = std::mem::take(&mut note.text);
        if !text.is_empty() {
            self.toast(now, "Cleared", Some(Undo::Cleared { id, title, text, password }));
            self.mark_dirty(now);
        }
    }

    // ---- encryption ---------------------------------------------------------------------------

    /// Encrypts the current note: from now on its text reaches the disk only as a cipher.
    pub fn encrypt_note(&mut self, password: &str, now: f64) -> Result<(), crypto::Error> {
        let note = self.state.current_mut();
        if note.is_locked() {
            return Err(crypto::Error::InvalidFormat);
        }
        note.encrypt(password)?;
        self.epoch += 1;
        let (id, hash) = (note.id, text_hash(&note.text));
        self.sealed.insert(id, hash);
        // Write the cipher over the plaintext the file held until now.
        if self.save_now() {
            self.toast(now, "Encrypted: saved only encrypted from now on", None);
        } else {
            self.save_error = Some("Encrypted in memory, but not saved — the disk copy may still be plaintext".into());
        }
        Ok(())
    }

    pub fn unlock_note(&mut self, password: &str, now: f64) -> Result<(), crypto::Error> {
        let note = self.state.current_mut();
        note.unlock(password)?;
        self.epoch += 1;
        let (id, hash) = (note.id, text_hash(&note.text));
        self.sealed.insert(id, hash);
        self.unlock_form = Default::default();
        self.focus_editor = true;
        self.toast(now, "Unlocked", None);
        Ok(())
    }

    /// Drops the plaintext and password of the current note from memory.
    pub fn lock_note(&mut self, now: f64) {
        let i = self.state.active;
        if !self.state.notes[i].is_encrypted() || self.state.notes[i].is_locked() {
            return;
        }
        self.seal_sync(i);
        let note = &mut self.state.notes[i];
        note.lock();
        self.epoch += 1;
        self.sealed.remove(&note.id);
        self.forget.push(note.id);
        // Replaces any Undo toast too: its plaintext must not outlive the lock.
        self.toast = None;
        if self.write() {
            self.toast(now, "Locked", None);
        }
    }

    /// Turns the current (unlocked) note back into a plain note.
    pub fn remove_encryption(&mut self, now: f64) {
        let note = self.state.current_mut();
        if !note.is_encrypted() || note.is_locked() {
            return;
        }
        note.remove_encryption();
        self.epoch += 1;
        let id = note.id;
        self.sealed.remove(&id);
        self.toast(now, "Encryption removed", None);
        self.mark_dirty(now);
    }

    pub fn showing_cipher(&self) -> bool {
        self.show_cipher == Some(self.state.current().id) && self.state.current().is_encrypted()
    }

    /// Shows the current note as the encrypted message saved to disk, or back as text.
    pub fn toggle_cipher_view(&mut self, now: f64) {
        if self.showing_cipher() {
            self.show_cipher = None;
            self.focus_editor = true;
            return;
        }
        if !self.state.current().is_encrypted() {
            self.toast(now, "The note is not encrypted", None);
            return;
        }
        // Show what the file holds for the text as it is now, not an older cipher.
        if self.is_stale(self.state.current()) {
            self.seal_sync(self.state.active);
            self.mark_dirty(now);
        }
        self.show_cipher = Some(self.state.current().id);
    }

    /// The lock button and Ctrl+Shift+L: encrypt a plain note, lock an open one, unlock a locked one.
    fn lock_action(&mut self, anchor: Rect, now: f64) {
        let note = self.state.current();
        if !note.is_encrypted() {
            self.encrypt_form = EncryptForm { note: note.id, grab: true, ..Default::default() };
            self.toggle_popup(Popup::Encrypt, anchor);
        } else if note.is_locked() {
            self.show_cipher = None;
            self.focus_editor = true;
        } else {
            self.lock_note(now);
        }
    }

    /// Opens the native save dialog on a worker thread so the window keeps drawing.
    /// An encrypted note is saved as its encrypted message: its plaintext never goes to disk.
    pub fn export(&mut self) {
        self.seal_sync(self.state.active);
        let note = self.state.current();
        let title = note.title.clone();
        let contents = note.cipher.clone().unwrap_or_else(|| note.text.clone());
        let (tx, rx) = mpsc::channel();
        self.saves = Some(rx);
        let run = move || {
            let name = format!("{}.txt", sanitize(&title));
            let picked = rfd::FileDialog::new()
                .set_title("Save note")
                .set_file_name(name)
                .add_filter("Text", &["txt", "md"])
                .add_filter("All files", &["*"])
                .save_file();
            if let Some(path) = picked {
                let _ = tx.send(std::fs::write(&path, contents).map(|_| path).map_err(|e| e.to_string()));
            }
        };
        // AppKit dialogs must run on the main thread; elsewhere a worker keeps the window drawing.
        if cfg!(target_os = "macos") {
            run();
        } else {
            std::thread::spawn(run);
        }
    }

    fn undo(&mut self, undo: Undo, now: f64) {
        match undo {
            Undo::Cleared { id, title, text, password } => match self.state.notes.iter().position(|n| n.id == id) {
                // Never overwrite something typed since: then the text comes back as its own note.
                // The same goes for a note locked or encrypted since: its state must not change.
                Some(i)
                    if self.state.notes[i].text.is_empty()
                        && !self.state.notes[i].is_locked()
                        && self.state.notes[i].password == password =>
                {
                    self.state.notes[i].text = text;
                    self.state.active = i;
                }
                _ => {
                    let mut note = Note::new(title);
                    note.id = self.state.fresh_id();
                    note.text = text;
                    // Text that was encrypted never comes back as plaintext on disk.
                    if let Some(pw) = password
                        && let Err(e) = note.encrypt(&pw)
                    {
                        eprintln!("scrpad: could not encrypt restored text: {e}");
                        return;
                    }
                    if note.is_encrypted() {
                        self.sealed.insert(note.id, text_hash(&note.text));
                    }
                    self.state.notes.push(note);
                    self.state.active = self.state.notes.len() - 1;
                }
            },
            Undo::Closed { index, note } => {
                let index = index.min(self.state.notes.len());
                // Closing the last note left an empty placeholder: replace it.
                if self.state.notes.len() == 1 && self.state.notes[0].text.is_empty() && !self.state.notes[0].is_encrypted() {
                    self.state.notes.clear();
                }
                let index = index.min(self.state.notes.len());
                self.state.notes.insert(index, note);
                self.state.active = index;
            }
        }
        self.toast = None;
        self.focus_editor = true;
        self.mark_dirty(now);
    }

    fn shortcuts(&mut self, ctx: &egui::Context, now: f64) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let cmd = |key| KeyboardShortcut::new(Modifiers::COMMAND, key);
        let cmd_shift = |key| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, key);
        if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::N)) || i.consume_shortcut(&cmd(Key::T))) {
            self.add_note(now);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::W))) {
            self.close_note(self.state.active, now);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&cmd(Key::S))) {
            self.export();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&cmd_shift(Key::C))) {
            self.copy(ctx, now);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&cmd_shift(Key::H))) {
            self.toggle_hidden(now);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&cmd_shift(Key::L))) {
            self.lock_action(self.lock_button, now);
        }
        let n = self.state.notes.len();
        // Shift first: consume_shortcut ignores extra modifiers, so Ctrl+Tab would also match it.
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab))) {
            self.state.active = (self.state.active + n - 1) % n;
            self.focus_editor = true;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::CTRL, Key::Tab))) {
            self.state.active = (self.state.active + 1) % n;
            self.focus_editor = true;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&cmd_shift(Key::E))) {
            self.toggle_cipher_view(now);
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.popup.is_some() {
                self.popup = None;
            } else if self.showing_cipher() {
                self.toggle_cipher_view(now);
            }
        }
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn window_frame(&self, ui: &egui::Ui, rect: Rect, t: &Tokens) {
        let p = ui.painter();
        // A soft vertical gradient: a mesh of two colours, clipped by the rounded body drawn on top of it.
        p.rect_filled(rect, t.radius, t.bg_bottom);
        let mut mesh = egui::Mesh::default();
        let fade =
            Rect::from_min_max(Pos2::new(rect.min.x + 1.0, rect.min.y + t.radius), Pos2::new(rect.max.x - 1.0, rect.max.y - t.radius));
        mesh.colored_vertex(fade.left_top(), t.bg_top);
        mesh.colored_vertex(fade.right_top(), t.bg_top);
        mesh.colored_vertex(fade.left_bottom(), t.bg_bottom);
        mesh.colored_vertex(fade.right_bottom(), t.bg_bottom);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 2, 3);
        p.add(mesh);
        p.rect_filled(
            Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + t.radius)),
            egui::CornerRadius { nw: t.radius as u8, ne: t.radius as u8, sw: 0, se: 0 },
            t.bg_top,
        );
        p.rect_stroke(rect, t.radius, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    }

    fn in_resize_band(&self, ui: &egui::Ui) -> bool {
        let window = ui.ctx().content_rect();
        ui.input(|i| i.pointer.press_origin()).is_some_and(|p| !window.shrink(RESIZE_EDGE).contains(p))
    }

    /// Lets the borderless window be resized from its edges and corners.
    fn resize_edges(&self, ui: &egui::Ui, rect: Rect) {
        let Some(pos) = ui.ctx().pointer_hover_pos() else { return };
        if !rect.contains(pos) {
            return;
        }
        let e = RESIZE_EDGE;
        let (l, r) = (pos.x < rect.left() + e, pos.x > rect.right() - e);
        let (t, b) = (pos.y < rect.top() + e, pos.y > rect.bottom() - e);
        let dir = match (l, r, t, b) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (_, true, true, _) => ResizeDirection::NorthEast,
            (true, _, _, true) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, ..) => ResizeDirection::West,
            (_, true, ..) => ResizeDirection::East,
            (_, _, true, _) => ResizeDirection::North,
            (_, _, _, true) => ResizeDirection::South,
            _ => return,
        };
        let icon = match dir {
            ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
            ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
            ResizeDirection::West | ResizeDirection::East => CursorIcon::ResizeHorizontal,
            _ => CursorIcon::ResizeVertical,
        };
        ui.ctx().set_cursor_icon(icon);
        if ui.input(|i| i.pointer.primary_pressed()) {
            ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, rect: Rect, t: &Tokens, now: f64) {
        // Empty header space moves the window.
        let drag = ui.interact(rect, Id::new("drag-header"), Sense::click_and_drag());
        // A press in the resize band belongs to resize_edges, not to the window drag.
        if drag.drag_started() && !self.in_resize_band(ui) {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        ui.painter().text(
            Pos2::new(rect.left() + PAD + 2.0, rect.center().y + 2.0),
            Align2::LEFT_CENTER,
            "Scratchpad",
            theme::semibold(14.5),
            t.title,
        );
        let close = Rect::from_center_size(Pos2::new(rect.right() - PAD - 10.0, rect.center().y + 2.0), Vec2::splat(24.0));
        let resp = ui.interact(close, Id::new("close"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close window"));
        let fill = if resp.hovered() { t.text_muted } else { t.close_fill };
        ui.painter().circle_filled(close.center(), 9.5, fill);
        icons::paint(ui, close, "x", 12.0, t.close_glyph);
        if resp.on_hover_text("Close").clicked() && self.save_now() {
            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
        }
        let pin = Rect::from_center_size(close.center() - Vec2::new(38.0, 0.0), Vec2::splat(28.0));
        let tint = if self.state.pinned { t.accent } else { t.icon_faint };
        let tip = if self.state.pinned { "Floating on top (click to unpin)" } else { "Float on top" };
        if icon_button(ui, pin, if self.state.pinned { "pin-filled" } else { "pin" }, 19.0, tint, t, "Float on top")
            .on_hover_text(tip)
            .clicked()
        {
            self.toggle_pin(ui.ctx(), now);
        }
    }

    fn tabs(&mut self, ui: &mut egui::Ui, rect: Rect, t: &Tokens, now: f64) {
        let drag = ui.interact(rect, Id::new("drag-tabs"), Sense::click_and_drag());
        if drag.drag_started() && !self.in_resize_band(ui) {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            self.add_note(now);
        }
        // Right side: add and more.
        let more = Rect::from_center_size(Pos2::new(rect.right() - PAD - 10.0, rect.center().y), Vec2::splat(28.0));
        let add = more.translate(Vec2::new(-34.0, 0.0));
        if icon_button(ui, add, "plus", 20.0, t.icon, t, "New note").on_hover_text("New note (Ctrl+N)").clicked() {
            self.add_note(now);
        }
        let more_resp = icon_button(ui, more, "ellipsis", 20.0, t.icon, t, "More").on_hover_text("More");
        if more_resp.clicked() {
            self.toggle_popup(Popup::More, more);
        }

        let strip = Rect::from_min_max(Pos2::new(rect.left() + PAD - 2.0, rect.top()), Pos2::new(add.left() - 6.0, rect.bottom()));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(strip).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.set_clip_rect(strip.intersect(ui.clip_rect()));
        let mut switch_to = None;
        let mut close = None;
        egui::ScrollArea::horizontal()
            .id_salt("tab-strip")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(&mut child, |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for i in 0..self.state.notes.len() {
                    match self.tab(ui, i, t, now) {
                        TabAction::None => {}
                        TabAction::Select => switch_to = Some(i),
                        TabAction::Close => close = Some(i),
                    }
                }
            });
        // Tabs that overflow fade out instead of being cut off at a hard edge.
        let fade = Rect::from_min_max(Pos2::new(strip.right() - 14.0, strip.top()), strip.right_bottom());
        let mut mesh = egui::Mesh::default();
        let bg = t.bg_top;
        mesh.colored_vertex(fade.left_top(), Color32::TRANSPARENT);
        mesh.colored_vertex(fade.right_top(), bg);
        mesh.colored_vertex(fade.left_bottom(), Color32::TRANSPARENT);
        mesh.colored_vertex(fade.right_bottom(), bg);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 2, 3);
        ui.painter().add(mesh);
        if let Some(i) = switch_to
            && self.state.active != i
        {
            self.state.active = i;
            self.focus_editor = true;
            self.mark_dirty(now);
        }
        if let Some(i) = close {
            self.close_note(i, now);
        }
    }

    fn tab(&mut self, ui: &mut egui::Ui, i: usize, t: &Tokens, now: f64) -> TabAction {
        let selected = i == self.state.active;
        let h = 28.0;
        // Inline rename.
        if let Some((idx, buf, grab)) = &mut self.renaming
            && *idx == i
        {
            let edit = egui::TextEdit::singleline(buf)
                .font(theme::semibold(14.0))
                .desired_width(110.0)
                .margin(egui::Margin::symmetric(8, 5))
                .background_color(t.tab_selected);
            let resp = ui.add(edit);
            if std::mem::take(grab) {
                resp.request_focus();
            }
            let commit = resp.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.renaming = None;
            } else if commit {
                let title = buf.trim().to_owned();
                if !title.is_empty() {
                    self.state.notes[i].title = title;
                }
                self.renaming = None;
                self.focus_editor = true;
                self.mark_dirty(now);
            }
            return TabAction::None;
        }

        let title = self.state.notes[i].title.clone();
        let font = if selected { theme::semibold(14.0) } else { theme::regular(14.0) };
        let color = if selected { t.text } else { t.text_muted };
        let galley = ui.painter().layout_no_wrap(title, font, color);
        let x_w = if selected { 20.0 } else { 0.0 };
        let lock_w = if self.state.notes[i].is_encrypted() { 16.0 } else { 0.0 };
        let size = Vec2::new(galley.size().x + 20.0 + x_w + lock_w, h);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &self.state.notes[i].title));
        if selected {
            ui.painter().rect_filled(rect, 7.0, t.tab_selected);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 7.0, t.hover);
        }
        if lock_w > 0.0 {
            let name = if self.state.notes[i].is_locked() { "lock" } else { "lock-open" };
            icons::paint(
                ui,
                Rect::from_center_size(Pos2::new(rect.left() + 16.0, rect.center().y), Vec2::splat(13.0)),
                name,
                13.0,
                t.secure,
            );
        }
        ui.painter().galley(Pos2::new(rect.left() + 10.0 + lock_w, rect.center().y - galley.size().y / 2.0), galley, color);
        let id = self.state.notes[i].id;
        if selected && self.scrolled_to != Some(id) {
            self.scrolled_to = Some(id);
            ui.scroll_to_rect(rect, None);
        }
        let mut action = TabAction::None;
        if selected {
            let x_rect = Rect::from_center_size(Pos2::new(rect.right() - 16.0, rect.center().y), Vec2::splat(18.0));
            let x = ui.interact(x_rect, Id::new(("tab-x", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            x.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close note"));
            if x.hovered() {
                ui.painter().rect_filled(x_rect, 4.0, t.hover);
            }
            icons::paint(ui, x_rect, "x", 13.0, if x.hovered() { t.text } else { t.text_muted });
            if x.on_hover_text("Close note (Ctrl+W)").clicked() {
                return TabAction::Close;
            }
        }
        if resp.double_clicked() {
            self.renaming = Some((i, self.state.notes[i].title.clone(), true));
        } else if resp.clicked() {
            action = TabAction::Select;
        } else if resp.middle_clicked() {
            action = TabAction::Close;
        }
        resp.on_hover_text("Double-click to rename");
        action
    }

    fn editor(&mut self, ui: &mut egui::Ui, rect: Rect, t: &Tokens, now: f64) {
        if self.showing_cipher() {
            self.cipher_view(ui, rect, t);
            return;
        }
        if self.state.current().is_locked() {
            self.unlock_view(ui, rect, t, now);
            return;
        }
        let font = match self.state.font {
            FontStyle::Sans => egui::FontId::proportional(self.state.font_size),
            FontStyle::Mono => egui::FontId::monospace(self.state.font_size - 1.0),
        };
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        let hidden = self.state.current().hidden;
        let focus = std::mem::take(&mut self.focus_editor);
        let placeholder = egui::RichText::new("Type anything. It saves by itself.").color(t.placeholder).font(font.clone());
        let mut changed = false;
        egui::ScrollArea::vertical().id_salt(("editor", self.state.active)).auto_shrink(false).show(&mut child, |ui| {
            let note = &mut self.state.notes[self.state.active];
            let mut mask = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
                // Hidden notes keep their line structure but show only dots; the text itself is untouched.
                let shown: String = text.as_str().chars().map(|c| if c == '\n' { '\n' } else { '•' }).collect();
                let mut job = egui::text::LayoutJob::simple(shown, font.clone(), t.text, wrap);
                job.wrap.max_width = wrap;
                ui.fonts_mut(|f| f.layout_job(job))
            };
            let mut edit = egui::TextEdit::multiline(&mut note.text)
                .id(Id::new(("editor", note.id)))
                .frame(egui::Frame::NONE)
                .font(font.clone())
                .text_color(t.text)
                .hint_text(placeholder)
                .desired_width(f32::INFINITY)
                .min_size(Vec2::new(rect.width(), rect.height()))
                .margin(egui::Margin::symmetric(0, 0))
                .lock_focus(true);
            if hidden {
                edit = edit.layouter(&mut mask);
            }
            let resp = edit.show(ui).response;
            if focus {
                resp.request_focus();
            }
            changed = resp.changed();
        });
        if changed {
            self.mark_dirty(now);
        }
        if hidden && self.state.current().text.is_empty() {
            ui.painter().text(Pos2::new(rect.right(), rect.top()), Align2::RIGHT_TOP, "hidden", theme::regular(11.0), t.placeholder);
        }
    }

    /// The note as it is saved: the SimpleTextCrypt message, selectable but read-only.
    fn cipher_view(&mut self, ui: &mut egui::Ui, rect: Rect, t: &Tokens) {
        let cipher = self.state.current().cipher.clone().unwrap_or_default();
        let label = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + 20.0));
        let icon = Rect::from_min_size(label.min, Vec2::splat(16.0));
        icons::paint(ui, icon, "lock", 14.0, t.secure);
        ui.painter().text(
            Pos2::new(icon.right() + 6.0, icon.center().y),
            Align2::LEFT_CENTER,
            "Encrypted, as saved to disk · Esc to go back",
            theme::regular(12.0),
            t.secure,
        );
        let body = Rect::from_min_max(Pos2::new(rect.left(), label.bottom() + 6.0), rect.max);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(body.intersect(ui.clip_rect()));
        egui::ScrollArea::vertical().id_salt("cipher-view").auto_shrink(false).show(&mut child, |ui| {
            let mut shown = cipher.as_str();
            let edit = egui::TextEdit::multiline(&mut shown)
                .id(Id::new("cipher-view"))
                .frame(egui::Frame::NONE)
                .font(egui::FontId::monospace((self.state.font_size - 2.0).max(10.0)))
                .text_color(t.text_muted)
                .desired_width(f32::INFINITY)
                .margin(egui::Margin::symmetric(0, 0));
            let resp = ui.add(edit);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Encrypted text"));
        });
    }

    /// A locked note shows only a password field: its text is not in memory at all.
    fn unlock_view(&mut self, ui: &mut egui::Ui, rect: Rect, t: &Tokens, now: f64) {
        let id = self.state.current().id;
        if self.unlock_form.0 != id {
            self.unlock_form = (id, String::new(), None);
        }
        let focus = std::mem::take(&mut self.focus_editor);
        let col = Rect::from_center_size(rect.center() - Vec2::new(0.0, 10.0), Vec2::new(rect.width().min(240.0), 120.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(col).layout(egui::Layout::top_down(egui::Align::Center)));
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        let ui = &mut child;
        let (icon, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
        icons::paint(ui, icon, "lock", 24.0, t.secure);
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Encrypted note").font(theme::semibold(14.0)).color(t.text));
        ui.add_space(8.0);
        let mut submit = false;
        ui.horizontal(|ui| {
            let field_w = (ui.available_width() - 76.0).max(80.0);
            let edit = egui::TextEdit::singleline(&mut self.unlock_form.1)
                .id(Id::new("unlock-password"))
                .password(true)
                .hint_text("Password")
                .background_color(t.bg_bottom)
                .desired_width(field_w)
                .margin(egui::Margin::symmetric(8, 5));
            let resp = ui.add(edit);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Password"));
            if focus {
                resp.request_focus();
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            if ui.add(egui::Button::new(egui::RichText::new("Unlock").font(theme::semibold(13.0)))).clicked() {
                submit = true;
            }
        });
        if let Some(err) = self.unlock_form.2 {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(err).font(theme::regular(12.5)).color(t.error));
        }
        ui.add_space(6.0);
        let link =
            egui::Button::new(egui::RichText::new("Show encrypted text").font(theme::regular(12.5)).color(t.text_muted)).frame(false);
        if ui.add(link).on_hover_text("Ctrl+Shift+E").clicked() {
            self.toggle_cipher_view(now);
        }
        if submit {
            let password = std::mem::take(&mut self.unlock_form.1);
            if let Err(e) = self.unlock_note(&password, now) {
                self.unlock_form.2 = Some(if e == crypto::Error::EmptyPassword { "Type the password" } else { "Wrong password" });
                self.focus_editor = true;
            }
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui, rect: Rect, t: &Tokens, now: f64) {
        let y = rect.center().y;
        let size = Vec2::splat(32.0);
        let mut x = rect.left() + PAD + 10.0;
        // "Aa" is text, not an icon glyph: it reads the same everywhere since the font is embedded.
        let aa = Rect::from_center_size(Pos2::new(x + 2.0, y), Vec2::new(34.0, 32.0));
        let resp = ui.interact(aa, Id::new("font"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Font"));
        if resp.hovered() || self.popup.is_some_and(|(p, _)| p == Popup::Font) {
            ui.painter().rect_filled(aa, 7.0, t.hover);
        }
        ui.painter().text(aa.center(), Align2::CENTER_CENTER, "Aa", theme::regular(16.0), t.text_muted);
        if resp.on_hover_text("Font").clicked() {
            self.toggle_popup(Popup::Font, aa);
        }
        x += 40.0;
        let hidden = self.state.current().hidden;
        let eye = Rect::from_center_size(Pos2::new(x, y), size);
        let (name, tip) = if hidden { ("eye-off", "Show note (Ctrl+Shift+H)") } else { ("eye", "Hide note (Ctrl+Shift+H)") };
        let tint = if hidden { t.accent } else { t.icon_faint };
        if icon_button(ui, eye, name, 20.0, tint, t, "Hide note").on_hover_text(tip).clicked() {
            self.toggle_hidden(now);
        }
        x += 38.0;
        // Encryption gets its own icon and colour: unlike hiding, it changes what reaches the disk.
        let lock = Rect::from_center_size(Pos2::new(x, y), size);
        self.lock_button = lock;
        let note = self.state.current();
        let (name, tint, tip) = match (note.is_encrypted(), note.is_locked()) {
            (false, _) => ("lock-open", t.icon_faint, "Encrypt note… (Ctrl+Shift+L)"),
            (true, false) if self.save_error.is_some() => ("lock", t.error, "Encrypted in memory; changes are not saved"),
            (true, false) => ("lock", t.secure, "Encrypted; saved only encrypted. Click to lock now (Ctrl+Shift+L)"),
            (true, true) => ("lock", t.secure, "Locked: type the password to unlock"),
        };
        let open = self.popup.is_some_and(|(p, _)| p == Popup::Encrypt);
        let resp = icon_button(ui, lock, name, 19.0, if open { t.secure } else { tint }, t, "Encryption");
        if resp.on_hover_text(tip).clicked() {
            self.lock_action(lock, now);
        }
        x += 38.0;
        let copy = Rect::from_center_size(Pos2::new(x, y), size);
        if icon_button(ui, copy, "files", 19.0, t.icon_faint, t, "Copy").on_hover_text("Copy all (Ctrl+Shift+C)").clicked() {
            self.copy(ui.ctx(), now);
        }
        x += 38.0;
        let save = Rect::from_center_size(Pos2::new(x, y), size);
        if icon_button(ui, save, "download", 19.0, t.icon_faint, t, "Save to file").on_hover_text("Save to file… (Ctrl+S)").clicked() {
            self.export();
        }
        let trash = Rect::from_center_size(Pos2::new(rect.right() - PAD - 10.0, y), size);
        if icon_button(ui, trash, "trash-2", 19.0, t.icon_faint, t, "Clear note").on_hover_text("Clear note").clicked() {
            self.clear(now);
        }
    }

    fn toggle_popup(&mut self, kind: Popup, anchor: Rect) {
        self.popup = match self.popup {
            Some((k, _)) if k == kind => None,
            _ => Some((kind, anchor)),
        };
    }

    fn popups(&mut self, ctx: &egui::Context, window: Rect, t: &Tokens, now: f64) {
        let Some((kind, anchor)) = self.popup else { return };
        let (pos, pivot) = match kind {
            Popup::Font | Popup::Encrypt => (anchor.left_top() - Vec2::new(0.0, 4.0), Align2::LEFT_BOTTOM),
            Popup::More => (anchor.right_bottom() + Vec2::new(0.0, 4.0), Align2::RIGHT_TOP),
        };
        let frame = egui::Frame::new()
            .fill(t.popup)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(8)
            .inner_margin(egui::Margin::same(5))
            .shadow(egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(70) });
        let area = egui::Area::new(Id::new("popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .pivot(pivot)
            .constrain_to(window.shrink(4.0))
            .show(ctx, |ui| {
                frame.show(ui, |ui| {
                    ui.set_min_width(150.0);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    match kind {
                        Popup::Font => self.font_menu(ui, t, now),
                        Popup::More => self.more_menu(ui, t, now),
                        Popup::Encrypt => self.encrypt_menu(ui, t, now),
                    }
                });
            });
        let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
            && ctx.pointer_interact_pos().is_some_and(|p| !area.response.rect.contains(p) && !anchor.contains(p));
        if clicked_outside {
            self.popup = None;
        }
    }

    fn font_menu(&mut self, ui: &mut egui::Ui, t: &Tokens, now: f64) {
        for (style, label) in [(FontStyle::Sans, "Sans – Inter"), (FontStyle::Mono, "Mono – JetBrains Mono")] {
            if menu_item(ui, t, label, self.state.font == style).clicked() {
                self.state.font = style;
                self.mark_dirty(now);
            }
        }
        separator(ui, t);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(format!("Size {:.0}", self.state.font_size)).color(t.text_muted).font(theme::regular(13.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (label, delta) in [("A+", 1.0), ("A−", -1.0)] {
                    let b = egui::Button::new(egui::RichText::new(label).font(theme::semibold(13.0)).color(t.text)).frame(false);
                    if ui.add(b).clicked() {
                        self.state.font_size = (self.state.font_size + delta).clamp(10.0, 32.0);
                        self.mark_dirty(now);
                    }
                }
            });
        });
    }

    /// Sets a password for the current note: typed twice, as there is no way to recover it.
    fn encrypt_menu(&mut self, ui: &mut egui::Ui, t: &Tokens, now: f64) {
        ui.set_width(220.0);
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let (icon, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
            icons::paint(ui, icon, "lock", 15.0, t.secure);
            ui.label(egui::RichText::new("Encrypt note").font(theme::semibold(13.5)).color(t.text));
        });
        ui.label(
            egui::RichText::new("The text is saved to disk only encrypted. The password is not stored and cannot be recovered.")
                .font(theme::regular(12.0))
                .color(t.text_muted),
        );
        let EncryptForm { password, again, error, grab, .. } = &mut self.encrypt_form;
        let mut submit = false;
        let mut next = false;
        for (buf, label) in [(password, "Password"), (again, "Repeat password")] {
            let edit = egui::TextEdit::singleline(buf)
                .id(Id::new(("encrypt-password", label)))
                .password(true)
                .hint_text(label)
                .background_color(t.bg_bottom)
                .desired_width(f32::INFINITY)
                .margin(egui::Margin::symmetric(8, 5));
            let resp = ui.add(edit);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, label));
            if label == "Password" && std::mem::take(grab) || label == "Repeat password" && std::mem::take(&mut next) {
                resp.request_focus();
            }
            // Enter in the first field moves on to the second; in the second it submits.
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if label == "Password" { next = true } else { submit = true }
            }
        }
        if let Some(e) = error {
            ui.label(egui::RichText::new(*e).font(theme::regular(12.5)).color(t.error));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new(egui::RichText::new("Encrypt").font(theme::semibold(13.0)))).clicked() {
                submit = true;
            }
        });
        if submit {
            let form = &mut self.encrypt_form;
            if form.note != self.state.current().id {
                // The popup belongs to another note: never encrypt whatever is active now.
                self.popup = None;
            } else if form.password.is_empty() {
                form.error = Some("Type a password");
            } else if form.password != form.again {
                form.error = Some("The passwords don't match");
            } else {
                let password = std::mem::take(&mut form.password);
                form.again.clear();
                match self.encrypt_note(&password, now) {
                    Ok(()) => {
                        self.popup = None;
                        self.focus_editor = true;
                    }
                    Err(e) => {
                        self.encrypt_form.error =
                            Some(if e == crypto::Error::EmptyPassword { "Type a password" } else { "Encryption failed" })
                    }
                }
            }
        }
    }

    fn more_menu(&mut self, ui: &mut egui::Ui, t: &Tokens, now: f64) {
        if menu_item(ui, t, "Rename note", false).clicked() {
            self.renaming = Some((self.state.active, self.state.current().title.clone(), true));
            self.popup = None;
        }
        if menu_item(ui, t, "Close note", false).clicked() {
            self.close_note(self.state.active, now);
            self.popup = None;
        }
        let note = self.state.current();
        if note.is_encrypted() && menu_item(ui, t, "Show encrypted text", self.showing_cipher()).clicked() {
            self.toggle_cipher_view(now);
            self.popup = None;
        }
        let note = self.state.current();
        if !note.is_encrypted() {
            if menu_item(ui, t, "Encrypt note…", false).clicked() {
                self.encrypt_form = EncryptForm { note: note.id, grab: true, ..Default::default() };
                self.popup = Some((Popup::Encrypt, self.lock_button));
            }
        } else if !note.is_locked() {
            if menu_item(ui, t, "Lock note", false).clicked() {
                self.lock_note(now);
                self.popup = None;
            }
            if menu_item(ui, t, "Remove encryption", false).clicked() {
                self.remove_encryption(now);
                self.popup = None;
            }
        }
        separator(ui, t);
        let other = match self.state.theme {
            ThemeKind::Dark => (ThemeKind::Light, "Light theme"),
            ThemeKind::Light => (ThemeKind::Dark, "Dark theme"),
        };
        if menu_item(ui, t, other.1, false).clicked() {
            self.state.theme = other.0;
            self.mark_dirty(now);
            self.popup = None;
        }
        if menu_item(ui, t, "Float on top", self.state.pinned).clicked() {
            self.toggle_pin(ui.ctx(), now);
        }
        separator(ui, t);
        let checking = self.update_check.is_some();
        if menu_item(ui, t, if checking { "Checking for updates…" } else { "Check for updates…" }, false).clicked() && !checking {
            self.check_for_updates(now);
            self.popup = None;
        }
        if menu_item(ui, t, "Quit", false).clicked() && self.save_now() {
            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
        }
        if self.save_error.is_some() && menu_item(ui, t, "Quit without saving", false).clicked() {
            self.dirty_since = None;
            self.force_close = true;
            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn toast_ui(&mut self, ui: &mut egui::Ui, above: Rect, t: &Tokens, now: f64) {
        let Some(toast) = &self.toast else { return };
        if now > toast.until {
            self.toast = None;
            return;
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(toast.until - now));
        let font = theme::regular(13.0);
        let text = ui.painter().layout_no_wrap(toast.text.clone(), font.clone(), t.text);
        let label = toast.action.as_ref().map(ToastAction::label);
        let action_label = label.map(|l| ui.painter().layout_no_wrap(l.into(), theme::semibold(13.0), t.accent));
        let w = text.size().x + 24.0 + action_label.as_ref().map_or(0.0, |g| g.size().x + 14.0);
        let rect = Rect::from_center_size(Pos2::new(above.center().x, above.bottom() - 22.0), Vec2::new(w, 28.0));
        let p = ui.painter();
        p.rect_filled(rect.translate(Vec2::new(0.0, 2.0)), 14.0, Color32::from_black_alpha(40));
        p.rect_filled(rect, 14.0, t.popup);
        p.rect_stroke(rect, 14.0, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        let text_w = text.size().x;
        p.galley(Pos2::new(rect.left() + 12.0, rect.center().y - text.size().y / 2.0), text, t.text);
        if let (Some(g), Some(label)) = (action_label, label) {
            let r = Rect::from_min_size(Pos2::new(rect.left() + 12.0 + text_w + 14.0, rect.center().y - g.size().y / 2.0), g.size());
            ui.painter().galley(r.min, g, t.accent);
            let resp = ui.interact(r.expand(4.0), Id::new("toast-action"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
            if resp.clicked() {
                match self.toast.take().and_then(|t| t.action) {
                    Some(ToastAction::Undo(undo)) => self.undo(undo, now),
                    Some(ToastAction::Open(url)) => ui.ctx().open_url(egui::OpenUrl::new_tab(url)),
                    None => {}
                }
            }
        }
    }

    fn save_error_ui(&self, ui: &mut egui::Ui, above: Rect, t: &Tokens) {
        let Some(error) = &self.save_error else { return };
        let suffix =
            if self.save_blocked { " Export your notes, then use More → Quit without saving." } else { " Retrying automatically." };
        let galley = ui.painter().layout(format!("{error}{suffix}"), theme::regular(12.5), t.error, (above.width() - 40.0).max(160.0));
        let rect = Rect::from_center_size(
            Pos2::new(above.center().x, above.bottom() - galley.size().y / 2.0 - 10.0),
            galley.size() + Vec2::new(24.0, 14.0),
        );
        ui.painter().rect_filled(rect, 8.0, t.popup);
        ui.painter().rect_stroke(rect, 8.0, Stroke::new(1.0, t.error), egui::StrokeKind::Inside);
        ui.painter().galley(rect.min + Vec2::new(12.0, 7.0), galley, t.error);
    }

    /// Sends a pending pin state to Hyprland and picks up pinning done with its own keys.
    #[cfg(target_os = "linux")]
    fn sync_wm(&mut self, ctx: &egui::Context, now: f64) {
        let Some(wm) = &mut self.wm else { return };
        if let Some((pinned, deadline)) = wm.pending {
            // The start-up request gets its deadline once the clock runs.
            let deadline = if deadline.is_nan() { now + 5.0 } else { deadline };
            wm.pending = Some((pinned, deadline));
            if wm.hypr.set_pinned(pinned) {
                wm.pending = None;
                wm.ignore_until = now + 1.5;
                if std::mem::take(&mut wm.restore) {
                    wm.hypr.place(self.state.window_size, self.state.window_pos);
                }
            } else if now < deadline {
                // Not mapped yet right after start: try again shortly.
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            } else {
                wm.pending = None;
            }
        }
        while let Ok(win) = wm.observed.try_recv() {
            if wm.pending.is_some() || now < wm.ignore_until {
                continue;
            }
            if win.pinned != self.state.pinned {
                self.state.pinned = win.pinned;
                self.dirty_since.get_or_insert(now);
            }
            // Only a floating window's geometry is worth remembering; a tiled one is the layout's.
            if win.floating {
                let size = Some([win.size[0] as f32, win.size[1] as f32]);
                let pos = Some([win.at[0] as f32, win.at[1] as f32]);
                if (size, pos) != (self.state.window_size, self.state.window_pos) {
                    self.state.window_size = size;
                    self.state.window_pos = pos;
                    self.dirty_since.get_or_insert(now);
                }
            }
        }
    }

    /// Remembers the window's size (and position where the platform reports it) for next start.
    fn track_geometry(&mut self, ctx: &egui::Context, now: f64) {
        #[cfg(target_os = "linux")]
        if self.wm.is_some() {
            return; // Hyprland reports it (and only for floating windows): see sync_wm.
        }
        let (inner, outer, odd) = ctx.input(|i| {
            let v = i.viewport();
            (v.inner_rect, v.outer_rect, v.minimized == Some(true) || v.maximized == Some(true) || v.fullscreen == Some(true))
        });
        let Some(inner) = inner.filter(|_| !odd) else { return };
        let size = Some([inner.width().round(), inner.height().round()]);
        // Wayland never reports a real position.
        let pos = outer.filter(|_| std::env::var_os("WAYLAND_DISPLAY").is_none()).map(|r| [r.min.x.round(), r.min.y.round()]);
        if size != self.state.window_size || (pos.is_some() && pos != self.state.window_pos) {
            self.state.window_size = size;
            self.state.window_pos = pos.or(self.state.window_pos);
            self.mark_dirty(now);
        }
    }

    fn poll_saves(&mut self, now: f64) {
        let Some(rx) = &self.saves else { return };
        match rx.try_recv() {
            Ok(Ok(path)) => {
                let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                self.saves = None;
                self.toast(now, format!("Saved {name}"), None);
            }
            Ok(Err(e)) => {
                self.saves = None;
                self.toast(now, format!("Save failed: {e}"), None);
            }
            Err(mpsc::TryRecvError::Disconnected) => self.saves = None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    /// Draws one frame into `ui` (the full window). Shared by eframe and the headless tests.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let t = self.tokens();
        if self.applied_theme != Some(t.kind) {
            t.apply(&ctx);
            self.applied_theme = Some(t.kind);
        }
        // Fonts given to set_fonts only exist from the next frame: named families would panic now.
        if !self.fonts_ready {
            egui_extras::install_image_loaders(&ctx);
            theme::install_fonts(&ctx);
            self.fonts_ready = true;
            ctx.request_repaint();
            return;
        }
        self.poll_saves(now);
        self.poll_update_check(&ctx, now);
        self.poll_sealed(&ctx, now);
        if self.saves.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        self.shortcuts(&ctx, now);
        #[cfg(target_os = "linux")]
        self.sync_wm(&ctx, now);
        self.track_geometry(&ctx, now);

        let window = ctx.content_rect();
        self.window_frame(ui, window, &t);
        let header = Rect::from_min_size(window.min, Vec2::new(window.width(), HEADER_H));
        let tabs = Rect::from_min_size(Pos2::new(window.left(), header.bottom()), Vec2::new(window.width(), TABS_H));
        let footer = Rect::from_min_max(Pos2::new(window.left(), window.bottom() - FOOTER_H), window.max);
        let divider_y = tabs.bottom() + 3.0;
        ui.painter().hline(window.x_range().shrink(1.0), divider_y, Stroke::new(1.0, t.divider));
        let editor =
            Rect::from_min_max(Pos2::new(window.left() + PAD + 4.0, divider_y + 10.0), Pos2::new(window.right() - PAD, footer.top()));

        self.header(ui, header, &t, now);
        self.tabs(ui, tabs, &t, now);
        self.editor(ui, editor, &t, now);
        self.footer(ui, footer, &t, now);
        let message_area = Rect::from_min_max(editor.min, Pos2::new(window.right(), footer.top()));
        self.toast_ui(ui, message_area, &t, now);
        if self.save_error.is_some() {
            // Above the toast, so an Undo stays reachable while saving fails.
            let above = if self.toast.is_some() { message_area.with_max_y(message_area.bottom() - 36.0) } else { message_area };
            self.save_error_ui(ui, above, &t);
        }
        self.popups(&ctx, window, &t, now);
        self.resize_edges(ui, window);
        if self.popup.is_some_and(|(p, _)| p == Popup::Encrypt) && self.encrypt_form.note != self.state.current().id {
            self.popup = None;
        }
        if !self.popup.is_some_and(|(p, _)| p == Popup::Encrypt) && self.encrypt_form != EncryptForm::default() {
            // A typed password does not linger once the popup is gone.
            self.encrypt_form = EncryptForm::default();
        }
        if self.show_cipher.is_some() && !self.showing_cipher() {
            self.show_cipher = None;
        }
        for id in self.forget.drain(..) {
            ctx.data_mut(|d| d.remove::<egui::text_edit::TextEditState>(Id::new(("editor", id))));
        }

        if let Some(since) = self.dirty_since {
            if self.save_blocked {
                // The original unreadable file could not be preserved; never overwrite it.
            } else if self.save_retry_at.is_some_and(|retry| retry > std::time::Instant::now()) {
                ctx.request_repaint_after(self.save_retry_at.expect("checked above").saturating_duration_since(std::time::Instant::now()));
            } else if now - since >= SAVE_DELAY {
                self.autosave(&ctx);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(SAVE_DELAY));
            }
        }
    }
}

/// The Encrypt popup: the note it was opened for, the password typed twice, an error to show,
/// and whether the first field still needs to grab focus.
#[derive(Default, PartialEq)]
struct EncryptForm {
    note: u64,
    password: String,
    again: String,
    error: Option<&'static str>,
    grab: bool,
}

/// A cipher finished by a sealing worker, and the hash of the plaintext it holds.
struct Sealed {
    id: u64,
    epoch: u64,
    hash: [u8; 32],
    cipher: Result<String, crypto::Error>,
}

struct SealJob {
    id: u64,
    epoch: u64,
    text: Zeroizing<String>,
    password: Zeroizing<String>,
}

/// One long-lived worker limits background Argon2id work to one 64 MiB job at a time.
fn spawn_sealer() -> (mpsc::Sender<SealJob>, mpsc::Receiver<Sealed>) {
    let (job_tx, job_rx) = mpsc::channel::<SealJob>();
    let (result_tx, result_rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(job) = job_rx.recv() {
            let hash = text_hash(&job.text);
            let cipher = crypto::encrypt(&job.password, &job.text);
            if result_tx.send(Sealed { id: job.id, epoch: job.epoch, hash, cipher }).is_err() {
                break;
            }
        }
    });
    (job_tx, result_rx)
}

fn text_hash(text: &str) -> [u8; 32] {
    Blake2s256::digest(text.as_bytes()).into()
}

enum TabAction {
    None,
    Select,
    Close,
}

/// A square icon button: transparent until hovered.
fn icon_button(ui: &mut egui::Ui, rect: Rect, icon: &str, size: f32, tint: Color32, t: &Tokens, label: &str) -> Response {
    let resp = ui.interact(rect, Id::new(("icon-btn", label)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if resp.hovered() {
        ui.painter().rect_filled(rect, 7.0, t.hover);
    }
    let shift = if resp.is_pointer_button_down_on() { Vec2::new(0.0, 1.0) } else { Vec2::ZERO };
    icons::paint(ui, rect.translate(shift), icon, size, tint);
    resp
}

fn menu_item(ui: &mut egui::Ui, t: &Tokens, label: &str, checked: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(150.0), 26.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, checked, label));
    if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, t.hover);
    }
    ui.painter().text(Pos2::new(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, label, theme::regular(13.5), t.text);
    if checked {
        ui.painter().circle_filled(Pos2::new(rect.right() - 12.0, rect.center().y), 3.5, t.accent);
    }
    resp
}

fn separator(ui: &mut egui::Ui, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 7.0), Sense::hover());
    ui.painter().hline(rect.x_range().shrink(4.0), rect.center().y, Stroke::new(1.0, t.divider));
}

/// A file name from a note title: no path separators or reserved characters.
fn sanitize(title: &str) -> String {
    let s: String = title.chars().map(|c| if r#"/\:*?"<>|"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    if s.trim().is_empty() { "note".into() } else { s.trim().to_owned() }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.force_close && !self.save_now() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // A sealing worker may still be running: its note is stale and gets sealed here.
        if !self.force_close && (self.dirty_since.is_some() || self.state.notes.iter().any(|n| self.is_stale(n))) {
            let _ = self.save_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{App, SealJob, sanitize, spawn_sealer};
    use crate::store::State;
    use zeroize::Zeroizing;

    #[test]
    fn sanitizes_file_names() {
        assert_eq!(sanitize("API keys / prod"), "API keys _ prod");
        assert_eq!(sanitize("  "), "note");
    }

    #[test]
    fn failed_encryption_save_stays_dirty_and_warns_about_plaintext() {
        let blocker = std::env::temp_dir().join(format!("scrpad-save-blocker-{}", std::process::id()));
        std::fs::write(&blocker, "not a directory").unwrap();
        let ctx = egui::Context::default();
        let mut app = App::with_state(&ctx, State::default(), Some(blocker.join("notes.json")));
        app.state.current_mut().text = "previously plaintext".into();
        app.encrypt_note("a long unique password", 1.0).unwrap();
        assert!(app.dirty_since.is_some());
        assert!(app.save_error.as_deref().is_some_and(|e| e.contains("disk copy may still be plaintext")));
        app.force_close = true;
        drop(app);
        let _ = std::fs::remove_file(&blocker);
    }

    #[test]
    fn recovering_from_a_save_error_keeps_the_undo_toast() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        state.add_note();
        state.notes[0].text = "keep me".into();
        let mut app = App::with_state(&ctx, state, None);
        app.close_note(0, 1.0);
        app.save_error = Some("Changes not saved: disk full".into());
        assert!(app.write());
        assert!(app.save_error.is_none());
        assert!(app.toast.as_ref().is_some_and(|t| matches!(t.action, Some(super::ToastAction::Undo(_)))));
    }

    #[test]
    fn an_available_update_offers_only_its_release_page() {
        let ctx = egui::Context::default();
        let mut app = App::with_state(&ctx, State::default(), None);
        let url = "https://github.com/mhavo/scrpad/releases/tag/v9.0.0".to_owned();
        app.show_update_outcome(crate::update::Outcome::Available { version: "9.0.0".into(), url: url.clone() }, 1.0);
        let toast = app.toast.as_ref().unwrap();
        assert_eq!(toast.text, "scrpad 9.0.0 is available");
        assert!(matches!(&toast.action, Some(super::ToastAction::Open(u)) if *u == url));
    }

    #[test]
    fn nothing_is_checked_until_the_user_asks() {
        let ctx = egui::Context::default();
        let app = App::with_state(&ctx, State::default(), None);
        assert!(app.update_check.is_none());
    }

    #[test]
    fn one_sealer_processes_multiple_jobs() {
        let (jobs, results) = spawn_sealer();
        for (id, text) in [(1, "first"), (2, "second")] {
            jobs.send(SealJob { id, epoch: 0, text: Zeroizing::new(text.into()), password: Zeroizing::new("pw".into()) }).unwrap();
        }
        assert_eq!([results.recv().unwrap().id, results.recv().unwrap().id], [1, 2]);
    }
}
