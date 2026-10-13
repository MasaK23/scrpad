//! AI writing help: hands text to an external helper (`ai-enhance --pipe <action>`, or the
//! command in `SCRPAD_AI_CMD`) on a worker thread and reads the result from its stdout.
//! Nothing runs on its own: suggestions only while the user has turned them on, rewrites only
//! when asked. Hidden and encrypted notes never reach the helper.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;

/// What the helper is asked to do; the name is the helper's action argument.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Complete,
    Improve,
    Fix,
    Shorten,
    Finnish,
    English,
}

impl Action {
    /// Everything the AI popup offers, with its label.
    pub const REWRITES: [(Action, &'static str); 5] = [
        (Action::Improve, "Improve writing"),
        (Action::Fix, "Fix spelling and grammar"),
        (Action::Shorten, "Make shorter"),
        (Action::Finnish, "Translate to Finnish"),
        (Action::English, "Translate to English"),
    ];

    fn arg(self) -> &'static str {
        match self {
            Action::Complete => "complete",
            Action::Improve => "improve",
            Action::Fix => "fix",
            Action::Shorten => "shorten",
            Action::Finnish => "fi",
            Action::English => "en",
        }
    }
}

/// Runs the helper on a worker thread; the receiver gets the result once it is done.
pub fn spawn(action: Action, text: String, ctx: egui::Context) -> mpsc::Receiver<Result<String, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run(action, &text));
        ctx.request_repaint();
    });
    rx
}

fn run(action: Action, text: &str) -> Result<String, String> {
    let cmd = std::env::var("SCRPAD_AI_CMD").unwrap_or_else(|_| "ai-enhance".into());
    let mut child = Command::new(&cmd)
        .args(["--pipe", action.arg()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {cmd}: {e}"))?;
    // Written from here and then closed, so the helper sees the end of its input.
    child.stdin.take().expect("piped").write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(if err.is_empty() { format!("{cmd} failed") } else { err });
    }
    let result = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if result.is_empty() { Err("the AI returned nothing".into()) } else { Ok(result) }
}

/// Joins a suggested continuation to the text: a space between words, none after a space or
/// newline, or before punctuation.
pub fn join(text: &str, suggestion: &str) -> String {
    let needs_space = text.chars().last().is_some_and(|c| !c.is_whitespace())
        && suggestion.chars().next().is_some_and(|c| !matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | '\'' | '"'));
    if needs_space { format!(" {suggestion}") } else { suggestion.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::join;

    #[test]
    fn joins_words_with_one_space() {
        assert_eq!(join("Hello", "world."), " world.");
        assert_eq!(join("Hello ", "world."), "world.");
        assert_eq!(join("Hello\n", "world."), "world.");
        assert_eq!(join("Hello", ", world."), ", world.");
        assert_eq!(join("", "Hello"), "Hello");
    }
}
