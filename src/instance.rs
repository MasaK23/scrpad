//! One scrpad per session: a second launch hands its wish to the running one over a Unix socket
//! in `$XDG_RUNTIME_DIR` (private to the user) and exits. `scrpad --toggle` bound to a key in
//! the compositor is how a hidden window comes back on Wayland, where apps get no global keys.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// A plain launch: bring the window here.
    Show,
    /// `--toggle`: hide the window when it has the focus, otherwise show it.
    Toggle,
}

impl Command {
    pub fn from_args(mut args: impl Iterator<Item = String>) -> Self {
        if args.any(|a| a == "--toggle") { Command::Toggle } else { Command::Show }
    }

    fn word(self) -> &'static str {
        match self {
            Command::Show => "show",
            Command::Toggle => "toggle",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        [Command::Show, Command::Toggle].into_iter().find(|c| c.word() == word.trim())
    }
}

pub fn socket_path() -> Option<PathBuf> {
    dirs::runtime_dir().map(|dir| dir.join("scrpad.sock"))
}

pub enum Claim {
    /// Another instance took the command: this one should exit.
    Forwarded,
    /// This is the only instance; commands for it arrive here.
    Listening(UnixListener),
}

/// Sends `command` to a running instance, or becomes the instance that listens.
pub fn claim(path: &Path, command: Command) -> std::io::Result<Claim> {
    if send(path, command).is_ok() {
        return Ok(Claim::Forwarded);
    }
    // Nobody answered: what is left is a socket from a crashed run, safe to replace.
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    UnixListener::bind(path).map(Claim::Listening)
}

fn send(path: &Path, command: Command) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    stream.write_all(format!("{}\n", command.word()).as_bytes())
}

/// Calls `on_command` for every command another launch sends, on a thread of its own.
pub fn serve(listener: UnixListener, on_command: impl Fn(Command) + Send + 'static) {
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok()
                && let Some(command) = Command::parse(&line)
            {
                on_command(command);
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
    fn toggle_flag_is_recognised() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>().into_iter();
        assert_eq!(Command::from_args(args(&["scrpad"])), Command::Show);
        assert_eq!(Command::from_args(args(&["scrpad", "--toggle"])), Command::Toggle);
    }

    #[test]
    fn second_launch_forwards_to_the_first() {
        let path = temp_socket("forward");
        let Claim::Listening(listener) = claim(&path, Command::Show).unwrap() else { panic!("no instance should be running") };
        let (tx, rx) = mpsc::channel();
        serve(listener, move |c| tx.send(c).unwrap());
        assert!(matches!(claim(&path, Command::Toggle).unwrap(), Claim::Forwarded));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Command::Toggle);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_stale_socket_is_replaced() {
        let path = temp_socket("stale");
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        assert!(matches!(claim(&path, Command::Show).unwrap(), Claim::Listening(_)));
        let _ = std::fs::remove_file(&path);
    }
}
