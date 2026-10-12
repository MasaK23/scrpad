//! One scrpad per session: a second launch hands its wish to the running one over a Unix socket
//! in `$XDG_RUNTIME_DIR` (private to the user) and exits. `scrpad --toggle` bound to a key in
//! the compositor is how a hidden window comes back on Wayland, where apps get no global keys.
//!
//! The protocol is one line, `show` or `toggle`, or `note <length>` followed by that many bytes
//! of UTF-8, which the running app acknowledges with `ok`. Every read is bounded in size and time.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cli::{Command, MAX_NOTE};

/// Longer than any command line (`note 1048576`).
const MAX_LINE: u64 = 32;
const TIMEOUT: Duration = Duration::from_secs(2);

fn encode(command: &Command) -> Vec<u8> {
    match command {
        Command::Show => b"show\n".to_vec(),
        Command::Toggle => b"toggle\n".to_vec(),
        Command::Note(text) => [format!("note {}\n", text.len()).as_bytes(), text.as_bytes()].concat(),
    }
}

/// Reads one command; `None` for anything malformed, too long or too slow.
fn decode(stream: &mut impl BufRead) -> Option<Command> {
    let mut line = String::new();
    stream.by_ref().take(MAX_LINE).read_line(&mut line).ok()?;
    let line = line.strip_suffix('\n')?;
    match line.split_once(' ') {
        None if line == "show" => Some(Command::Show),
        None if line == "toggle" => Some(Command::Toggle),
        Some(("note", len)) => {
            let len = len.parse::<usize>().ok().filter(|&len| len <= MAX_NOTE)?;
            let mut bytes = vec![0; len];
            stream.read_exact(&mut bytes).ok()?;
            String::from_utf8(bytes).ok().map(Command::Note)
        }
        _ => None,
    }
}

pub fn socket_path() -> Option<PathBuf> {
    dirs::runtime_dir().map(|dir| dir.join("scrpad.sock"))
}

pub enum Claim {
    /// Another instance took the command: this one should exit.
    Forwarded,
    /// Another instance runs but did not take the command (an older version, say).
    Refused(std::io::Error),
    /// This is the only instance; commands for it arrive here.
    Listening(UnixListener),
}

/// Sends `command` to a running instance, or becomes the instance that listens.
pub fn claim(path: &Path, command: &Command) -> std::io::Result<Claim> {
    if let Ok(stream) = UnixStream::connect(path) {
        return Ok(match send(stream, command) {
            Ok(()) => Claim::Forwarded,
            Err(e) => Claim::Refused(e),
        });
    }
    // Nobody answered: what is left is a socket from a crashed run, safe to replace.
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    UnixListener::bind(path).map(Claim::Listening)
}

/// A note must not get lost silently, so it waits for the running app to acknowledge it.
fn send(mut stream: UnixStream, command: &Command) -> std::io::Result<()> {
    stream.set_write_timeout(Some(TIMEOUT))?;
    stream.write_all(&encode(command))?;
    if let Command::Note(_) = command {
        stream.set_read_timeout(Some(TIMEOUT))?;
        let mut reply = String::new();
        BufReader::new(stream).take(MAX_LINE).read_line(&mut reply)?;
        if reply != "ok\n" {
            return Err(std::io::Error::other("the running scrpad did not take the note; restart it if it is an older version"));
        }
    }
    Ok(())
}

/// Calls `on_command` for every command another launch sends, on a thread of its own.
pub fn serve(listener: UnixListener, on_command: impl Fn(Command) + Send + 'static) {
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(TIMEOUT));
            let _ = stream.set_write_timeout(Some(TIMEOUT));
            let mut reader = BufReader::new(stream);
            if let Some(command) = decode(&mut reader) {
                let note = matches!(command, Command::Note(_));
                on_command(command);
                if note {
                    let _ = reader.get_mut().write_all(b"ok\n");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn temp_socket(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scrpad-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scrpad.sock");
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn commands_round_trip_and_garbage_is_dropped() {
        for command in [Command::Show, Command::Toggle, Command::Note(String::new()), Command::Note("två\nrader".into())] {
            assert_eq!(decode(&mut &encode(&command)[..]), Some(command));
        }
        // What older versions send.
        assert_eq!(decode(&mut &b"show\n"[..]), Some(Command::Show));
        for garbage in [
            &b"show"[..],
            b"shout\n",
            b"note\n",
            b"note -1\n",
            b"note 5\nabc",
            b"note 2\n\xff\xfe",
            format!("note {}\n", MAX_NOTE + 1).as_bytes(),
            &[b'x'; 1000],
        ] {
            assert_eq!(decode(&mut &garbage[..]), None, "{:?}", String::from_utf8_lossy(garbage));
        }
    }

    #[test]
    fn second_launch_forwards_to_the_first() {
        let path = temp_socket("forward");
        let Claim::Listening(listener) = claim(&path, &Command::Show).unwrap() else { panic!("no instance should be running") };
        let (tx, rx) = mpsc::channel();
        serve(listener, move |c| tx.send(c).unwrap());
        assert!(matches!(claim(&path, &Command::Toggle).unwrap(), Claim::Forwarded));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Command::Toggle);
        let note = Command::Note("x".repeat(MAX_NOTE));
        assert!(matches!(claim(&path, &note).unwrap(), Claim::Forwarded));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), note);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_note_an_older_instance_ignores_is_refused_not_lost() {
        let path = temp_socket("older");
        let listener = UnixListener::bind(&path).unwrap();
        // Reads one line and hangs up, as versions without notes do.
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = BufReader::new(stream).read_line(&mut String::new());
            }
        });
        assert!(matches!(claim(&path, &Command::Note("keep me".into())).unwrap(), Claim::Refused(_)));
        assert!(path.exists(), "the running instance keeps its socket");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_stale_socket_is_replaced() {
        let path = temp_socket("stale");
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        assert!(matches!(claim(&path, &Command::Show).unwrap(), Claim::Listening(_)));
        let _ = std::fs::remove_file(&path);
    }
}
