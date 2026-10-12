//! The command line: `scrpad [--toggle | --new | - | --append TITLE TEXT | --list | --read TITLE]`.

use std::io::Read;

/// The longest note `scrpad -` takes from stdin, and the longest text `--append` adds.
pub const MAX_NOTE: usize = 1 << 20;
/// The longest title `--append` and `--read` take, in bytes.
pub const MAX_TITLE: usize = 1024;

pub const HELP: &str = "\
scrpad: a tiny floating scratchpad

Usage: scrpad [--toggle | --new | - | --append TITLE TEXT | --list | --read TITLE]

  (nothing)            show the window
  --toggle             hide the window when it has the focus, show it otherwise
  --new                open a new empty note
  -                    open a new note with the text from stdin (UTF-8, at most 1 MiB)
  --append TITLE TEXT  add TEXT as a new paragraph to the note titled TITLE, made if missing;
                       TEXT - reads it from stdin (UTF-8, at most 1 MiB)
  --list               print the titles of the notes scripts may read, one per line
  --read TITLE         print the text of the first readable note titled TITLE
  -h, --help           show this help

A running scrpad takes over: it shows the window and adds the note. --append, --list and
--read leave the window as it is, and work on the notes file when no scrpad runs.
--list and --read need \"Let scripts read notes\" in the … menu; hidden and encrypted
notes are never listed or read, and --append refuses an encrypted note.
";

/// What a launch asks the app for, which may be one already running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// A plain launch: bring the window here.
    Show,
    /// `--toggle`: hide the window when it has the focus, otherwise show it.
    Toggle,
    /// `--new` (empty) and `-` (stdin): add a note with this text and bring the window here.
    Note(String),
    /// `--append`: add `text` as a paragraph to the note titled `title`, without showing anything.
    Append { title: String, text: String },
    /// `--list`: the titles of the readable notes.
    List,
    /// `--read`: the text of a readable note.
    Read(String),
}

impl Command {
    /// For scripts: carried out on the notes file when no window runs, never starting one.
    pub fn is_script(&self) -> bool {
        matches!(self, Command::Append { .. } | Command::List | Command::Read(_))
    }
}

/// The command `args` (without the program name) ask for; `Ok(None)` for `--help`. Reads
/// `stdin` only for `-` and `--append TITLE -`.
pub fn parse(args: impl Iterator<Item = String>, stdin: impl Read) -> Result<Option<Command>, String> {
    let mut args = args.peekable();
    let mut wish = None;
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().ok_or_else(|| format!("{arg} needs {what} (see scrpad --help)"));
        let command = match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--toggle" => Wish::Toggle,
            "--new" => Wish::New,
            "-" => Wish::Stdin,
            "--append" => {
                let title = value("a title and a text")?;
                Wish::Append(title, value("a text, or - for stdin")?)
            }
            "--list" => Wish::List,
            "--read" => Wish::Read(value("a title")?),
            // Finder adds a process serial number when it starts an app on older macOS.
            psn if psn.starts_with("-psn_") => continue,
            other => return Err(format!("unknown argument “{other}” (see scrpad --help)")),
        };
        if wish.replace(command).is_some() {
            return Err("give only one command (see scrpad --help)".into());
        }
    }
    Ok(Some(match wish {
        None => Command::Show,
        Some(Wish::Toggle) => Command::Toggle,
        Some(Wish::New) => Command::Note(String::new()),
        Some(Wish::Stdin) => Command::Note(read_note(stdin)?),
        Some(Wish::Append(title, text)) => {
            check_title(&title)?;
            let text = if text == "-" { read_note(stdin)? } else { text };
            check_text(&text)?;
            Command::Append { title, text }
        }
        Some(Wish::List) => Command::List,
        Some(Wish::Read(title)) => {
            check_title(&title)?;
            Command::Read(title)
        }
    }))
}

/// A command as given, before stdin is read.
enum Wish {
    Toggle,
    New,
    Stdin,
    Append(String, String),
    List,
    Read(String),
}

/// A title is one line of at most `MAX_TITLE` bytes, as the rename field makes them.
pub fn check_title(title: &str) -> Result<(), String> {
    if title.is_empty() {
        Err("the title is empty".into())
    } else if title.len() > MAX_TITLE {
        Err(format!("the title is longer than {MAX_TITLE} bytes"))
    } else if title.chars().any(char::is_control) {
        Err("the title holds a line break or another control character".into())
    } else {
        Ok(())
    }
}

/// What `--append` adds: something, and at most `MAX_NOTE` bytes.
pub fn check_text(text: &str) -> Result<(), String> {
    if text.is_empty() {
        Err("there is no text to append".into())
    } else if text.len() > MAX_NOTE {
        Err("the text is longer than 1 MiB".into())
    } else {
        Ok(())
    }
}

/// At most `MAX_NOTE` bytes of UTF-8 text, without the one trailing newline `echo` adds.
pub fn read_note(input: impl Read) -> Result<String, String> {
    let mut bytes = Vec::new();
    input.take(MAX_NOTE as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("could not read stdin: {e}"))?;
    if bytes.len() > MAX_NOTE {
        return Err("stdin holds more than 1 MiB; a note takes at most that".into());
    }
    let mut text = String::from_utf8(bytes).map_err(|_| "stdin is not UTF-8 text".to_owned())?;
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_with(args: &[&str], stdin: &[u8]) -> Result<Option<Command>, String> {
        parse(args.iter().map(|s| s.to_string()), stdin)
    }

    fn append(title: &str, text: &str) -> Command {
        Command::Append { title: title.into(), text: text.into() }
    }

    #[test]
    fn flags_are_recognised() {
        assert_eq!(parse_with(&[], b""), Ok(Some(Command::Show)));
        assert_eq!(parse_with(&["--toggle"], b""), Ok(Some(Command::Toggle)));
        assert_eq!(parse_with(&["--new"], b"ignored"), Ok(Some(Command::Note(String::new()))));
        assert_eq!(parse_with(&["-"], b"sk-live-TOKEN\n"), Ok(Some(Command::Note("sk-live-TOKEN".into()))));
        assert_eq!(parse_with(&["--help"], b""), Ok(None));
        assert_eq!(parse_with(&["-psn_0_12345"], b""), Ok(Some(Command::Show)));
        assert!(parse_with(&["--bogus"], b"").unwrap_err().contains("--bogus"));
        assert!(parse_with(&["--new", "-"], b"").is_err());
    }

    #[test]
    fn script_commands_take_their_values() {
        assert_eq!(parse_with(&["--append", "Agent", "cargo test"], b"ignored"), Ok(Some(append("Agent", "cargo test"))));
        assert_eq!(parse_with(&["--append", "Agent", "-"], b"ls -la\n"), Ok(Some(append("Agent", "ls -la"))));
        assert_eq!(parse_with(&["--list"], b""), Ok(Some(Command::List)));
        assert_eq!(parse_with(&["--read", "Todo"], b""), Ok(Some(Command::Read("Todo".into()))));
        // A value that looks like a flag is still the value.
        assert_eq!(parse_with(&["--read", "--list"], b""), Ok(Some(Command::Read("--list".into()))));
        assert!([Command::List, Command::Read("x".into()), append("x", "y")].iter().all(Command::is_script));
        assert!(![Command::Show, Command::Toggle, Command::Note("x".into())].iter().any(Command::is_script));
    }

    #[test]
    fn script_commands_refuse_bad_values() {
        assert!(parse_with(&["--append"], b"").unwrap_err().contains("needs a title"));
        assert!(parse_with(&["--append", "Agent"], b"").unwrap_err().contains("needs a text"));
        assert!(parse_with(&["--read"], b"").unwrap_err().contains("needs a title"));
        assert!(parse_with(&["--list", "--read", "x"], b"").unwrap_err().contains("only one"));
        assert!(parse_with(&["--append", "", "x"], b"").unwrap_err().contains("empty"));
        assert!(parse_with(&["--append", "two\nlines", "x"], b"").unwrap_err().contains("line break"));
        assert!(parse_with(&["--read", &"t".repeat(MAX_TITLE + 1)], b"").unwrap_err().contains("longer"));
        assert!(parse_with(&["--append", "Agent", ""], b"").unwrap_err().contains("no text"));
        assert!(parse_with(&["--append", "Agent", "-"], b"\n").unwrap_err().contains("no text"));
        assert!(parse_with(&["--append", "Agent", &"a".repeat(MAX_NOTE + 1)], b"").unwrap_err().contains("1 MiB"));
        assert!(parse_with(&["--append", "Agent", "-"], &vec![b'a'; MAX_NOTE + 1]).unwrap_err().contains("1 MiB"));
        assert!(parse_with(&["--append", "Agent", "-"], &[0xff]).unwrap_err().contains("UTF-8"));
        assert!(check_title(&"t".repeat(MAX_TITLE)).is_ok());
    }

    #[test]
    fn stdin_is_capped_and_must_be_utf8() {
        assert_eq!(read_note(&b"two\nlines\r\n"[..]).unwrap(), "two\nlines");
        assert_eq!(read_note(&b"keeps\n\n"[..]).unwrap(), "keeps\n");
        assert_eq!(read_note(&vec![b'a'; MAX_NOTE][..]).unwrap().len(), MAX_NOTE);
        assert!(read_note(&vec![b'a'; MAX_NOTE + 1][..]).unwrap_err().contains("1 MiB"));
        assert!(read_note(&[0xff, 0xfe][..]).unwrap_err().contains("UTF-8"));
    }
}
