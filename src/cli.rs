//! The command line: `scrpad [--toggle | --new | -]`.

use std::io::Read;

/// The longest note `scrpad -` takes from stdin.
pub const MAX_NOTE: usize = 1 << 20;

pub const HELP: &str = "\
scrpad: a tiny floating scratchpad

Usage: scrpad [--toggle | --new | -]

  (nothing)   show the window
  --toggle    hide the window when it has the focus, show it otherwise
  --new       open a new empty note
  -           open a new note with the text from stdin (UTF-8, at most 1 MiB)
  -h, --help  show this help

On Linux a running scrpad takes over: it shows the window and adds the note.
";

/// What a launch asks the app for, which on Linux may be one already running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// A plain launch: bring the window here.
    Show,
    /// `--toggle`: hide the window when it has the focus, otherwise show it.
    Toggle,
    /// `--new` (empty) and `-` (stdin): add a note with this text and bring the window here.
    Note(String),
}

/// The command `args` (without the program name) ask for; `Ok(None)` for `--help`. Reads
/// `stdin` only for `-`.
pub fn parse(args: impl Iterator<Item = String>, stdin: impl Read) -> Result<Option<Command>, String> {
    let mut wish = None;
    for arg in args {
        let command = match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--toggle" => "--toggle",
            "--new" => "--new",
            "-" => "-",
            // Finder adds a process serial number when it starts an app on older macOS.
            psn if psn.starts_with("-psn_") => continue,
            other => return Err(format!("unknown argument “{other}” (see scrpad --help)")),
        };
        if wish.replace(command).is_some() {
            return Err("give only one of --toggle, --new and - (see scrpad --help)".into());
        }
    }
    Ok(Some(match wish {
        None => Command::Show,
        Some("--toggle") => Command::Toggle,
        Some("--new") => Command::Note(String::new()),
        Some(_) => Command::Note(read_note(stdin)?),
    }))
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
    fn stdin_is_capped_and_must_be_utf8() {
        assert_eq!(read_note(&b"two\nlines\r\n"[..]).unwrap(), "two\nlines");
        assert_eq!(read_note(&b"keeps\n\n"[..]).unwrap(), "keeps\n");
        assert_eq!(read_note(&vec![b'a'; MAX_NOTE][..]).unwrap().len(), MAX_NOTE);
        assert!(read_note(&vec![b'a'; MAX_NOTE + 1][..]).unwrap_err().contains("1 MiB"));
        assert!(read_note(&[0xff, 0xfe][..]).unwrap_err().contains("UTF-8"));
    }
}
