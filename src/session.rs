//! What the app does, apart from how it looks: the notes, saving, sealing encrypted notes,
//! locking and hiding, toasts and the requests of other launches. No UI toolkit in here: a
//! frontend draws it and calls in.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::Duration;

use blake2::{Blake2s256, Digest};
use zeroize::Zeroizing;

use crate::crypto;
use crate::scripts;
use crate::store::{Note, State};
use crate::update;

const SAVE_DELAY: f64 = 0.4;
const SAVE_RETRY_DELAY: Duration = Duration::from_secs(3);
const TOAST_SECS: f64 = 4.0;
/// Long enough to read the version and reach the Download button.
const UPDATE_TOAST_SECS: f64 = 12.0;
/// Without input for this long, encrypted notes lock and revealed notes hide again (if enabled).
const IDLE_SECS: f64 = 5.0 * 60.0;

/// Asks the frontend to run its loop again after the given time (zero: right away). Called from
/// worker threads too.
pub type Wake = Arc<dyn Fn(Duration) + Send + Sync>;

/// What an Undo in the toast brings back.
pub(crate) enum Undo {
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

pub(crate) struct Toast {
    pub(crate) text: String,
    pub(crate) until: f64,
    pub(crate) action: Option<ToastAction>,
}

/// The button a toast can carry.
pub(crate) enum ToastAction {
    Undo(Undo),
    /// Opens a release page in the browser.
    Open(String),
}

impl Undo {
    /// Plaintext of an encrypted note, which must not outlive locking.
    fn holds_secret(&self) -> bool {
        match self {
            Undo::Cleared { password, .. } => password.is_some(),
            Undo::Closed { note, .. } => note.is_encrypted() && !note.is_locked(),
        }
    }
}

impl ToastAction {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            ToastAction::Undo(_) => "Undo",
            ToastAction::Open(_) => "Download",
        }
    }
}

pub struct Session {
    pub state: State,
    pub(crate) path: Option<PathBuf>,
    pub(crate) dirty_since: Option<f64>,
    save_retry_at: Option<std::time::Instant>,
    pub(crate) save_error: Option<String>,
    pub(crate) save_blocked: bool,
    pub(crate) toast: Option<Toast>,
    pub(crate) saves: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    /// A "Check for updates…" the user started, still waiting for GitHub.
    pub(crate) update_check: Option<mpsc::Receiver<update::Outcome>>,
    /// Asks the editor to take keyboard focus on the next frame (after switching or adding a tab).
    pub(crate) focus_editor: bool,
    /// Per encrypted note: a hash of the plaintext its `cipher` holds. Differs from the text: stale.
    sealed: HashMap<u64, [u8; 32]>,
    /// Encrypted notes being re-sealed on a worker thread (Argon2id takes a noticeable moment).
    sealing: HashSet<u64>,
    seal_jobs: mpsc::Sender<SealJob>,
    seal_rx: mpsc::Receiver<Sealed>,
    /// Bumped on every encrypt, lock, unlock and decrypt: sealing results from before are void.
    epoch: u64,
    /// Notes whose editor state (and with it the undo history of their plaintext) must go.
    pub(crate) forget: Vec<u64>,
    /// The encrypted note whose cipher is shown in place of its text (read-only).
    pub(crate) show_cipher: Option<u64>,
    /// Hidden notes revealed with the eye in this session: hidden again on idle or hiding the window.
    revealed: HashSet<u64>,
    /// When the user last gave any input (the frontend's clock), for the idle timeout.
    last_input: f64,
    /// `--toggle` asking a focused (so visible and drawing) window to hide.
    pub(crate) hide_requests: (mpsc::Sender<()>, mpsc::Receiver<()>),
    /// Notes from `scrpad --new` and `scrpad -` launched while this one runs.
    pub(crate) note_requests: (mpsc::Sender<String>, mpsc::Receiver<String>),
    /// Shared with the thread that answers `scrpad --append`, `--list` and `--read`.
    pub(crate) scripts: Arc<Mutex<scripts::Window>>,
    wake: Wake,
}

impl Session {
    pub fn new(mut state: State, path: Option<PathBuf>, startup_error: Option<String>, wake: Wake) -> Self {
        state.fix_up();
        let (seal_jobs, seal_rx) = spawn_sealer();
        let sealed = state.notes.iter().filter(|n| n.is_encrypted() && !n.is_locked()).map(|n| (n.id, text_hash(&n.text))).collect();
        let save_blocked = startup_error.is_some();
        let session = Self {
            state,
            path,
            dirty_since: None,
            save_retry_at: None,
            save_error: startup_error,
            save_blocked,
            toast: None,
            saves: None,
            update_check: None,
            focus_editor: true,
            sealed,
            sealing: HashSet::new(),
            seal_jobs,
            seal_rx,
            epoch: 0,
            forget: Vec::new(),
            show_cipher: None,
            revealed: HashSet::new(),
            last_input: 0.0,
            hide_requests: mpsc::channel(),
            note_requests: mpsc::channel(),
            scripts: Arc::new(Mutex::new(scripts::Window { blocked: save_blocked, ..Default::default() })),
            wake,
        };
        lock(&session.scripts).track(&session.state);
        session
    }

    fn repaint(&self) {
        (self.wake)(Duration::ZERO);
    }

    fn repaint_after(&self, delay: Duration) {
        (self.wake)(delay);
    }

    pub(crate) fn mark_dirty(&mut self, now: f64) {
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
    fn autosave(&mut self) {
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
            self.repaint_after(Duration::from_millis(100));
        }
        if worker_ok {
            if !self.write() {
                self.repaint();
                self.repaint_after(SAVE_RETRY_DELAY);
            }
        } else {
            self.dirty_since.get_or_insert(0.0);
            self.save_error = Some("Changes not saved: encryption worker stopped".into());
            self.repaint();
        }
    }

    fn write(&mut self) -> bool {
        if self.save_blocked {
            return false;
        }
        // Saved under the scripts lock, with the appends scripts saved meanwhile added first: an
        // append already in the file is never overwritten by a save without it.
        let shared = self.scripts.clone();
        let mut window = lock(&shared);
        self.take_appends(&mut window, None);
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

    pub(crate) fn is_stale(&self, note: &Note) -> bool {
        note.is_encrypted() && !note.is_locked() && self.sealed.get(&note.id) != Some(&text_hash(&note.text))
    }

    /// Brings the cipher of note `i` up to date on this thread.
    pub(crate) fn seal_sync(&mut self, i: usize) {
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
    pub fn poll_sealed(&mut self, now: f64) {
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
            self.repaint_after(Duration::from_millis(100));
        }
    }

    pub(crate) fn toast(&mut self, now: f64, text: impl Into<String>, undo: Option<Undo>) {
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

    pub fn poll_update_check(&mut self, now: f64) {
        let Some(rx) = &self.update_check else { return };
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            Err(mpsc::TryRecvError::Empty) => {
                self.repaint_after(Duration::from_millis(200));
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

    /// A new note holding `text` (`scrpad -`), titled like any new note.
    pub fn add_note_with_text(&mut self, text: String, now: f64) {
        self.add_note(now);
        self.state.current_mut().text = text;
    }

    /// Adds what scripts appended to the notes file to the notes here (`scrpad --append`), in
    /// the background: the active note, the focus and the window stay as they are. A toast says
    /// so, unless it would take the place of one with an Undo.
    pub(crate) fn take_appends(&mut self, window: &mut scripts::Window, now: Option<f64>) {
        for (title, text) in std::mem::take(&mut window.appends) {
            // Decided as for the file, which this window has the last word on.
            if let Err(e) = scripts::append(&mut self.state, &title, &text) {
                eprintln!("scrpad: {e}");
                continue;
            }
            self.mark_dirty(now.unwrap_or(0.0));
            if let Some(now) = now
                && self.toast.as_ref().is_none_or(|t| t.action.is_none())
            {
                self.toast(now, format!("Added to “{title}”"), None);
            }
        }
        window.track(&self.state);
    }

    /// Answers `--append`, `--list` and `--read` from another launch, from the notes file, on
    /// the thread that serves the socket. The frontend is woken to take in what was appended.
    pub(crate) fn script_server(&self) -> impl Fn(&crate::cli::Command) -> crate::instance::Reply + Send + 'static {
        let (window, path, wake) = (self.scripts.clone(), self.path.clone(), self.wake.clone());
        move |command| {
            let mut window = lock(&window);
            let reply = scripts::answer(path.as_deref(), command, Some(&mut window));
            let added = !window.appends.is_empty();
            drop(window);
            if added {
                wake(Duration::ZERO);
            }
            reply
        }
    }

    pub fn close_note(&mut self, i: usize, now: f64) {
        let note = self.state.notes[i].clone();
        self.state.close_note(i);
        self.forget.push(note.id);
        if !note.text.is_empty() || note.is_encrypted() {
            let msg = format!("Closed “{}”", note.title);
            self.toast(now, msg, Some(Undo::Closed { index: i, note }));
        }
        self.mark_dirty(now);
    }

    pub fn toggle_hidden(&mut self, now: f64) {
        let note = self.state.current_mut();
        note.hidden = !note.hidden;
        let (id, hidden) = (note.id, note.hidden);
        if hidden {
            self.revealed.remove(&id);
        } else {
            self.revealed.insert(id);
        }
        self.mark_dirty(now);
    }

    /// The user stepped away (the window hides, or no input for `IDLE_SECS`): locks encrypted
    /// notes and hides revealed ones again, as the options ask. Returns how many notes it locked.
    pub(crate) fn step_away(&mut self, now: f64) -> usize {
        let locked = if self.state.auto_lock { self.lock_all() } else { 0 };
        if self.state.auto_rehide {
            let mut rehidden = false;
            for note in &mut self.state.notes {
                if self.revealed.remove(&note.id) && !note.hidden {
                    note.hidden = true;
                    rehidden = true;
                }
            }
            self.revealed.clear();
            if rehidden {
                self.mark_dirty(now);
            }
        }
        locked
    }

    /// Whether stepping away would lock or re-hide anything.
    fn exposed(&self) -> bool {
        let notes = &self.state.notes;
        self.state.auto_lock && notes.iter().any(|n| n.is_encrypted() && !n.is_locked())
            || self.state.auto_rehide && notes.iter().any(|n| !n.hidden && self.revealed.contains(&n.id))
    }

    /// Steps away after `IDLE_SECS` without input. Runs before anything is drawn, so a window
    /// that was not drawing (hidden, or on another workspace) never shows the text first.
    pub fn check_idle(&mut self, now: f64, input: bool) {
        if now - self.last_input >= IDLE_SECS && self.exposed() {
            // Counted from here on: a note that could not be sealed is retried later, not every frame.
            self.last_input = now;
            if self.step_away(now) > 0 {
                self.toast(now, "Locked after 5 minutes without input", None);
            }
        }
        if input {
            self.last_input = now;
        }
        if self.exposed() {
            self.repaint_after(Duration::from_secs_f64((self.last_input + IDLE_SECS - now).max(0.0)));
        }
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
        self.focus_editor = true;
        self.toast(now, "Unlocked", None);
        Ok(())
    }

    /// Seals note `i` and drops its plaintext, password and editor undo history from memory.
    /// A note whose cipher could not be brought up to date stays open: locking it would lose
    /// what was typed since. Returns whether it locked.
    fn lock_at(&mut self, i: usize) -> bool {
        if !self.state.notes[i].is_encrypted() || self.state.notes[i].is_locked() {
            return false;
        }
        self.seal_sync(i);
        if self.is_stale(&self.state.notes[i]) {
            return false;
        }
        let note = &mut self.state.notes[i];
        note.lock();
        self.sealed.remove(&note.id);
        self.forget.push(note.id);
        true
    }

    /// Drops the plaintext and password of the current note from memory.
    pub fn lock_note(&mut self, now: f64) {
        if !self.lock_at(self.state.active) {
            return;
        }
        self.epoch += 1;
        // Replaces any Undo toast too: its plaintext must not outlive the lock.
        self.toast = None;
        if self.write() {
            self.toast(now, "Locked", None);
        }
    }

    /// Locks every unlocked encrypted note; returns how many.
    pub fn lock_all(&mut self) -> usize {
        let locked = (0..self.state.notes.len()).filter(|&i| self.lock_at(i)).count();
        // An Undo for a closed or cleared encrypted note holds its plaintext too.
        if self.toast.as_ref().is_some_and(|t| matches!(&t.action, Some(ToastAction::Undo(u)) if u.holds_secret())) {
            self.toast = None;
        }
        if locked > 0 {
            self.epoch += 1;
            self.write();
        }
        locked
    }

    /// `…` → Lock all and Ctrl+Shift+K: whatever the options say.
    pub fn lock_all_now(&mut self, now: f64) {
        match self.lock_all() {
            0 if self.state.notes.iter().any(|n| n.is_encrypted() && !n.is_locked()) => {} // The save error says why.
            0 => self.toast(now, "No unlocked notes", None),
            1 => self.toast(now, "Locked 1 note", None),
            n => self.toast(now, format!("Locked {n} notes"), None),
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

    pub(crate) fn undo(&mut self, undo: Undo, now: f64) {
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

    pub(crate) fn poll_saves(&mut self, now: f64) {
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

    /// The debounced autosave, once the changes have settled for `SAVE_DELAY`.
    pub fn autosave_when_due(&mut self, now: f64) {
        if let Some(since) = self.dirty_since {
            if self.save_blocked {
                // The original unreadable file could not be preserved; never overwrite it.
            } else if self.save_retry_at.is_some_and(|retry| retry > std::time::Instant::now()) {
                self.repaint_after(self.save_retry_at.expect("checked above").saturating_duration_since(std::time::Instant::now()));
            } else if now - since >= SAVE_DELAY {
                self.autosave();
            } else {
                self.repaint_after(Duration::from_secs_f64(SAVE_DELAY));
            }
        }
    }
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

pub(crate) fn text_hash(text: &str) -> [u8; 32] {
    Blake2s256::digest(text.as_bytes()).into()
}

/// The lock shared with the thread that answers scripts. A panic there leaves nothing half done
/// here: the notes file is only ever replaced whole.
pub(crate) fn lock(window: &Mutex<scripts::Window>) -> MutexGuard<'_, scripts::Window> {
    window.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::{SealJob, Session, spawn_sealer};
    use crate::store::State;
    use std::sync::Arc;
    use zeroize::Zeroizing;

    #[test]
    fn failed_encryption_save_stays_dirty_and_warns_about_plaintext() {
        let blocker = std::env::temp_dir().join(format!("scrpad-save-blocker-{}", std::process::id()));
        std::fs::write(&blocker, "not a directory").unwrap();
        let mut session = Session::new(State::default(), Some(blocker.join("notes.json")), None, Arc::new(|_| {}));
        session.state.current_mut().text = "previously plaintext".into();
        session.encrypt_note("a long unique password", 1.0).unwrap();
        assert!(session.dirty_since.is_some());
        assert!(session.save_error.as_deref().is_some_and(|e| e.contains("disk copy may still be plaintext")));
        drop(session);
        let _ = std::fs::remove_file(&blocker);
    }

    #[test]
    fn recovering_from_a_save_error_keeps_the_undo_toast() {
        let mut state = State::default();
        state.add_note();
        state.notes[0].text = "keep me".into();
        let mut session = Session::new(state, None, None, Arc::new(|_| {}));
        session.close_note(0, 1.0);
        session.save_error = Some("Changes not saved: disk full".into());
        assert!(session.write());
        assert!(session.save_error.is_none());
        assert!(session.toast.as_ref().is_some_and(|t| matches!(t.action, Some(super::ToastAction::Undo(_)))));
    }

    #[test]
    fn an_available_update_offers_only_its_release_page() {
        let mut session = Session::new(State::default(), None, None, Arc::new(|_| {}));
        let url = "https://github.com/mhavo/scrpad/releases/tag/v9.0.0".to_owned();
        session.show_update_outcome(crate::update::Outcome::Available { version: "9.0.0".into(), url: url.clone() }, 1.0);
        let toast = session.toast.as_ref().unwrap();
        assert_eq!(toast.text, "scrpad 9.0.0 is available");
        assert!(matches!(&toast.action, Some(super::ToastAction::Open(u)) if *u == url));
    }

    #[test]
    fn nothing_is_checked_until_the_user_asks() {
        let session = Session::new(State::default(), None, None, Arc::new(|_| {}));
        assert!(session.update_check.is_none());
    }

    #[test]
    fn idle_locks_and_hides_again_only_once_the_time_is_up() {
        let mut state = State::default();
        state.add_note();
        state.add_note();
        state.notes[0].text = "secret".into();
        state.notes[0].encrypt("pw").unwrap();
        state.notes[1].hidden = true;
        state.active = 1;
        let mut session = Session::new(state, None, None, Arc::new(|_| {}));
        session.toggle_hidden(1.0);
        assert!(!session.state.notes[1].hidden);
        session.check_idle(super::IDLE_SECS - 1.0, false);
        assert!(!session.state.notes[0].is_locked() && !session.state.notes[1].hidden);
        session.check_idle(super::IDLE_SECS + 1.0, false);
        assert!(session.state.notes[0].is_locked() && session.state.notes[0].text.is_empty());
        assert!(session.state.notes[1].hidden);
        // Never hidden by the user: left alone.
        assert!(!session.state.notes[2].hidden);
        assert_eq!(crate::crypto::decrypt("pw", session.state.notes[0].cipher.as_ref().unwrap()).unwrap(), "secret");
    }

    #[test]
    fn idle_does_nothing_with_the_options_off() {
        let mut state = State { auto_lock: false, auto_rehide: false, ..State::default() };
        state.notes[0].encrypt("pw").unwrap();
        state.notes[0].hidden = true;
        let mut session = Session::new(state, None, None, Arc::new(|_| {}));
        session.toggle_hidden(1.0);
        session.check_idle(10.0 * super::IDLE_SECS, false);
        assert!(!session.state.notes[0].is_locked() && !session.state.notes[0].hidden);
    }

    #[test]
    fn lock_all_drops_an_undo_that_holds_plaintext() {
        let mut state = State::default();
        state.add_note();
        state.notes[1].text = "secret".into();
        state.notes[1].encrypt("pw").unwrap();
        let mut session = Session::new(state, None, None, Arc::new(|_| {}));
        session.close_note(1, 1.0);
        assert!(session.toast.is_some());
        assert_eq!(session.lock_all(), 0);
        assert!(session.toast.is_none());
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
