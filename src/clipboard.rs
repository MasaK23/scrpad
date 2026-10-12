//! Takes a copied secret off the clipboard after a while, but only while the clipboard still
//! holds exactly that text: whatever was copied since is never wiped.
//!
//! egui sets the clipboard (`ctx.copy_text`) but cannot read it back, so reading and clearing
//! go through arboard (X11, macOS, Windows) and, on Wayland, the data-control protocol
//! (wlroots compositors such as Hyprland and Sway, KDE). A Wayland compositor without it gets
//! no clearing at all rather than a blind one.

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use zeroize::Zeroizing;

/// How long a copied secret stays on the clipboard.
pub const CLEAR_AFTER: Duration = Duration::from_secs(30);

/// The system clipboard, as far as clearing it needs.
pub trait Clipboard {
    /// The text on the clipboard; `None` when it holds none or cannot be read.
    fn text(&mut self) -> Option<Zeroizing<String>>;
    fn clear(&mut self) -> bool;
}

/// Clears the clipboard if its text still hashes to `copied`. Returns whether it cleared.
pub fn clear_if_unchanged(clipboard: &mut dyn Clipboard, copied: &[u8; 32]) -> bool {
    clipboard.text().is_some_and(|text| crate::app::text_hash(&text) == *copied) && clipboard.clear()
}

/// Opens the clipboard; `None` where it cannot be read.
pub type Open = dyn Fn() -> Option<Box<dyn Clipboard>> + Send + Sync;

/// The copy a clear is due for and the hash of its secret (never the secret itself).
type Pending = Mutex<Option<(u64, [u8; 32])>>;

/// The clear still due for the secret copied last.
pub struct Watch {
    open: Arc<Open>,
    delay: Duration,
    pending: Arc<Pending>,
    copies: u64,
    /// Whether the clipboard can be read here, found out on the first secret copied.
    readable: Option<bool>,
}

impl Watch {
    pub fn new(open: Arc<Open>, delay: Duration) -> Self {
        Self { open, delay, pending: Arc::default(), copies: 0, readable: None }
    }

    pub fn system() -> Self {
        Self::new(Arc::new(system), CLEAR_AFTER)
    }

    /// After copying a secret: clears it after the delay unless something else was copied by
    /// then. Returns false where the clipboard cannot be read, so it will not be cleared.
    pub fn secret_copied(&mut self, text: &str) -> bool {
        let open = self.open.clone();
        if !*self.readable.get_or_insert_with(|| open().is_some()) {
            return false;
        }
        self.copies += 1;
        let copy = self.copies;
        *lock(&self.pending) = Some((copy, crate::app::text_hash(text)));
        let (pending, delay) = (self.pending.clone(), self.delay);
        // A thread, not a frame timer: a hidden window draws no frames.
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            clear_pending(&*open, &pending, Some(copy));
        });
        true
    }

    /// After copying anything else: there is no secret to clear any more.
    pub fn other_copied(&mut self) {
        *lock(&self.pending) = None;
    }

    pub fn is_pending(&self) -> bool {
        lock(&self.pending).is_some()
    }

    /// On quit: clears a secret still due, waiting at most `timeout` for the clipboard.
    pub fn clear_now(&self, timeout: Duration) {
        if !self.is_pending() {
            return;
        }
        let (open, pending) = (self.open.clone(), self.pending.clone());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(clear_pending(&*open, &pending, None));
        });
        let _ = rx.recv_timeout(timeout);
    }
}

/// Clears the pending secret, if it is still the one from `copy` (any one for `None`).
fn clear_pending(open: &Open, pending: &Pending, copy: Option<u64>) -> bool {
    let hash = {
        let mut pending = lock(pending);
        match *pending {
            Some((current, hash)) if copy.is_none_or(|copy| copy == current) => {
                *pending = None;
                hash
            }
            _ => return false,
        }
    };
    open().is_some_and(|mut clipboard| clear_if_unchanged(clipboard.as_mut(), &hash))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn system() -> Option<Box<dyn Clipboard>> {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return wayland::DataControl::open().map(|c| Box::new(c) as Box<dyn Clipboard>);
    }
    arboard::Clipboard::new().ok().map(|c| Box::new(c) as Box<dyn Clipboard>)
}

impl Clipboard for arboard::Clipboard {
    fn text(&mut self) -> Option<Zeroizing<String>> {
        self.get_text().ok().map(Zeroizing::new)
    }

    fn clear(&mut self) -> bool {
        arboard::Clipboard::clear(self).is_ok()
    }
}

/// Wayland lets a client read and clear the clipboard without the keyboard focus only through
/// data-control; arboard would fall back to the X11 clipboard of XWayland, which can be stale.
#[cfg(target_os = "linux")]
mod wayland {
    use std::io::Read;

    use wl_clipboard_rs::{copy, paste, utils};
    use zeroize::Zeroizing;

    /// More than a note from stdin (1 MiB) holds; a longer text is not ours.
    const LIMIT: u64 = 4 << 20;

    pub struct DataControl;

    impl DataControl {
        /// `None` when the compositor lacks data-control.
        pub fn open() -> Option<Self> {
            utils::is_primary_selection_supported().ok().map(|_| DataControl)
        }
    }

    impl super::Clipboard for DataControl {
        fn text(&mut self) -> Option<Zeroizing<String>> {
            let (pipe, _) = paste::get_contents(paste::ClipboardType::Regular, paste::Seat::Unspecified, paste::MimeType::Text).ok()?;
            let mut bytes = Vec::new();
            pipe.take(LIMIT).read_to_end(&mut bytes).ok()?;
            String::from_utf8(bytes).ok().map(Zeroizing::new)
        }

        fn clear(&mut self) -> bool {
            copy::clear(copy::ClipboardType::Regular, copy::Seat::All).is_ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clipboard in memory; `None` stands for one that cannot be read.
    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<Option<String>>>);

    impl Clipboard for Fake {
        fn text(&mut self) -> Option<Zeroizing<String>> {
            lock(&self.0).clone().map(Zeroizing::new)
        }
        fn clear(&mut self) -> bool {
            *lock(&self.0) = Some(String::new());
            true
        }
    }

    fn holding(text: &str) -> Fake {
        Fake(Arc::new(Mutex::new(Some(text.into()))))
    }

    fn hash(text: &str) -> [u8; 32] {
        crate::app::text_hash(text)
    }

    #[test]
    fn clears_only_what_we_copied() {
        let mut clipboard = holding("sk-live-SECRET");
        assert!(clear_if_unchanged(&mut clipboard, &hash("sk-live-SECRET")));
        assert_eq!(lock(&clipboard.0).as_deref(), Some(""));
        // Copied something else since: left alone.
        let mut clipboard = holding("an address the user copied");
        assert!(!clear_if_unchanged(&mut clipboard, &hash("sk-live-SECRET")));
        assert_eq!(lock(&clipboard.0).as_deref(), Some("an address the user copied"));
        // Unreadable: never cleared blindly.
        let mut clipboard = Fake::default();
        assert!(!clear_if_unchanged(&mut clipboard, &hash("sk-live-SECRET")));
        assert_eq!(*lock(&clipboard.0), None);
    }

    fn watch(clipboard: &Fake, delay: Duration) -> Watch {
        let clipboard = clipboard.clone();
        Watch::new(Arc::new(move || Some(Box::new(clipboard.clone()) as Box<dyn Clipboard>)), delay)
    }

    fn eventually(done: impl Fn() -> bool) -> bool {
        (0..200).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            done()
        })
    }

    #[test]
    fn a_later_copy_cancels_the_clear() {
        let clipboard = holding("secret");
        let mut w = watch(&clipboard, Duration::from_millis(20));
        assert!(w.secret_copied("secret"));
        assert!(eventually(|| lock(&clipboard.0).as_deref() == Some("")));
        assert!(!w.is_pending());

        // A second secret replaces the first, and a plain copy cancels the clear altogether.
        *lock(&clipboard.0) = Some("second".into());
        let mut w = watch(&clipboard, Duration::from_millis(50));
        w.secret_copied("first");
        w.secret_copied("second");
        w.other_copied();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(lock(&clipboard.0).as_deref(), Some("second"));
    }

    #[test]
    fn quitting_clears_a_pending_secret() {
        let clipboard = holding("secret");
        let mut w = watch(&clipboard, Duration::from_secs(3600));
        w.secret_copied("secret");
        w.clear_now(Duration::from_secs(2));
        assert_eq!(lock(&clipboard.0).as_deref(), Some(""));
        assert!(!w.is_pending());
    }

    #[test]
    fn an_unreadable_clipboard_is_never_scheduled() {
        let mut w = Watch::new(Arc::new(|| None), Duration::from_millis(1));
        assert!(!w.secret_copied("secret"));
        assert!(!w.is_pending());
    }
}
