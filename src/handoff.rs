//! Handing a note to an agent's project without the agent seeing it: `scrpad --write` puts the
//! note in a file, `scrpad --run` puts its `KEY=VALUE` lines in a command's environment. Both run
//! only after the user allows them in scrpad (see `Session::allow`). Nothing here ever puts a
//! note's text in an error message: an agent reads those.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::store::{create_unique_temp, sync_parent};

/// The longest file name most file systems take, in bytes.
const MAX_NAME: usize = 255;

/// The note's title is the file's name: one plain name, never a path.
pub fn check_file_name(title: &str) -> Result<(), String> {
    if title.is_empty() || title == "." || title == ".." {
        Err(format!("“{title}” is not a file name"))
    } else if title.len() > MAX_NAME {
        Err(format!("a file name takes at most {MAX_NAME} bytes"))
    } else if title.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':')) {
        Err(format!("“{title}” holds a / \\ : or a control character; the title must be a plain file name"))
    } else {
        Ok(())
    }
}

/// Where `--write` would put a note, as the dialog shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    /// A file is there already: the dialog asks to replace it.
    pub exists: bool,
}

/// `dir/title`, if scrpad may write it: `dir` an existing absolute folder, and the name free or a
/// plain file (never a link, which could point anywhere, or a folder).
pub fn target(dir: &Path, title: &str) -> Result<Target, String> {
    check_file_name(title)?;
    if !dir.is_absolute() {
        return Err(format!("{} is not an absolute path", dir.display()));
    }
    if !std::fs::metadata(dir).is_ok_and(|m| m.is_dir()) {
        return Err(format!("{} is not a folder", dir.display()));
    }
    let path = dir.join(title);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Target { path, exists: false }),
        Err(e) => Err(format!("{}: {e}", path.display())),
        Ok(m) if m.file_type().is_symlink() => Err(format!("{} is a link; scrpad does not write through links", path.display())),
        Ok(m) if m.is_file() => Ok(Target { path, exists: true }),
        Ok(_) => Err(format!("{} is not a file", path.display())),
    }
}

/// Writes `text` (with a final newline) to `path`, private to the user and atomically: a temp
/// file next to it, then a rename. A rename replaces a link put there since `target`; it never
/// follows it. Without `replace`, a hard link puts the file in place only if nothing is there.
pub fn write(path: &Path, text: &str, replace: bool) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("could not write {}: {e}", path.display());
    let mut tmp = create_unique_temp(path).map_err(fail)?;
    tmp.file_mut().write_all(text.as_bytes()).map_err(fail)?;
    if !text.ends_with('\n') {
        tmp.file_mut().write_all(b"\n").map_err(fail)?;
    }
    tmp.file_mut().sync_all().map_err(fail)?;
    tmp.close();
    if replace {
        std::fs::rename(tmp.path(), path).map_err(fail)?;
        tmp.persisted();
    } else {
        match std::fs::hard_link(tmp.path(), path) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(format!("{} exists now; ask again to replace it", path.display()));
            }
            result => result.map_err(fail)?,
        }
        // `tmp` removes its own name when dropped; the file stays under `path`.
    }
    sync_parent(path).map_err(fail)
}

/// Whether git would leave `path` out of commits: `None` outside a repository or without git.
/// A local `git check-ignore`, nothing more.
pub fn git_ignored(path: &Path) -> Option<bool> {
    let (dir, name) = (path.parent()?, path.file_name()?);
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["check-ignore", "-q", "--"])
        .arg(name)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_dir(name: &str) -> PathBuf {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let n = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("scrpad-handoff-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(dir).unwrap()
    }

    #[test]
    fn only_plain_file_names_are_targets() {
        for ok in [".env", "config.json", "a b.txt", "ü.toml"] {
            assert!(check_file_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", ".", "..", "../x", "a/b", "a\\b", "C:x", "nul\0", &"x".repeat(256)] {
            assert!(check_file_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_target_is_checked_before_asking() {
        let dir = temp_dir("target");
        let t = target(&dir, ".env").unwrap();
        assert_eq!((t.path.clone(), t.exists), (dir.join(".env"), false));
        std::fs::write(dir.join(".env"), "OLD=1\n").unwrap();
        assert!(target(&dir, ".env").unwrap().exists);
        std::fs::create_dir(dir.join("sub")).unwrap();
        assert!(target(&dir, "sub").unwrap_err().contains("not a file"));
        assert!(target(&dir.join("missing"), ".env").unwrap_err().contains("folder"));
        assert!(target(Path::new("relative"), ".env").unwrap_err().contains("absolute"));
        assert!(target(&dir, "../.env").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", dir.join("link")).unwrap();
            assert!(target(&dir, "link").unwrap_err().contains("link"));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn writing_is_private_atomic_and_replaces_only_when_told() {
        let dir = temp_dir("write");
        let path = dir.join(".env");
        write(&path, "A=1", false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "A=1\n", "a final newline is added");
        assert!(write(&path, "A=2", false).unwrap_err().contains("exists"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "A=1\n");
        write(&path, "A=2\n", true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "A=2\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // No temp file left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_planted_after_the_dialog_is_never_followed() {
        let dir = temp_dir("race");
        let victim = dir.join("victim");
        std::fs::write(&victim, "keep me").unwrap();
        let path = dir.join(".env");
        let t = target(&dir, ".env").unwrap();
        std::os::unix::fs::symlink(&victim, &path).unwrap();
        // Not replacing: the link counts as an existing entry.
        assert!(write(&t.path, "S=1", false).is_err());
        // Replacing: the link itself is replaced, its target untouched.
        write(&t.path, "S=1", true).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        assert!(!std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn git_ignored_is_unknown_outside_a_repository() {
        let dir = temp_dir("git");
        assert_eq!(git_ignored(&dir.join(".env")), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
