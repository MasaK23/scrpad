#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Set at runtime because eframe otherwise puts its own egui icon on the window, the Windows
/// taskbar and the macOS Dock and Cmd-Tab switcher, over the one in the .app bundle.
/// macOS gets the icon with the padding of Apple's icon grid, elsewhere it fills its square.
#[cfg(target_os = "macos")]
const ICON: &[u8] = include_bytes!("../assets/app-icon/scrpad.png");
#[cfg(not(target_os = "macos"))]
const ICON: &[u8] = include_bytes!("../assets/app-icon/scrpad-256.png");

use scrpad::instance::{Claim, Reply};

fn main() -> eframe::Result {
    let command = match scrpad::cli::parse(std::env::args().skip(1), std::io::stdin()) {
        Ok(Some(command)) => command,
        Ok(None) => {
            print!("{}", scrpad::cli::HELP);
            return Ok(());
        }
        Err(error) => {
            eprintln!("scrpad: {error}");
            std::process::exit(2);
        }
    };
    // A running scrpad takes over a second launch (and `--toggle` from a compositor key).
    let address = scrpad::instance::address();
    let listener = match &address {
        Some(address) => match scrpad::instance::claim(address, &command) {
            Ok(Claim::Answered(reply)) => std::process::exit(finish(reply)),
            Ok(Claim::Refused(error)) => {
                // Starting a second instance would have two windows overwrite each other's notes.
                eprintln!("scrpad: {error}");
                std::process::exit(1);
            }
            Ok(Claim::Listening(listener)) => Some(listener),
            Err(error) => {
                eprintln!("scrpad: {}: {error}", address.display());
                None
            }
        },
        None => None,
    };
    if command.is_script() {
        // No window runs: the notes file is changed or read right here, and no window starts.
        let reply = match listener {
            // Held until done, answering `busy`: a scrpad started meanwhile waits, then loads the
            // file with the append in it, instead of loading it before and saving over it later.
            Some(listener) => scrpad::instance::hold(listener, address.as_deref().expect("a listener has an address"), || {
                scrpad::scripts::answer(scrpad::store::State::default_path().as_deref(), &command, None)
            }),
            // Without the socket a running window can't be told from none, and would save over the append.
            None if matches!(command, scrpad::cli::Command::Append { .. }) => {
                Reply::Error("no socket to reach a running scrpad, so nothing was appended (a window could save over it)".into())
            }
            None => scrpad::scripts::answer(scrpad::store::State::default_path().as_deref(), &command, None),
        };
        std::process::exit(finish(reply));
    }
    let configured_path = scrpad::store::State::default_path();
    let (state, path, startup_error) = match configured_path {
        Some(path) => match scrpad::store::State::load(&path) {
            Ok(state) => (state, Some(path), None),
            Err(error) => {
                eprintln!("scrpad: {error}");
                (
                    scrpad::store::State::default(),
                    None,
                    Some(format!("Could not preserve the unreadable notes file: {error}. The original was left untouched.")),
                )
            }
        },
        None => (scrpad::store::State::default(), None, None),
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Scratchpad")
        .with_app_id("scrpad")
        .with_icon(eframe::icon_data::from_png_bytes(ICON).expect("the embedded icon is a valid PNG"))
        .with_decorations(false)
        .with_transparent(true)
        .with_inner_size(state.window_size.unwrap_or([380.0, 300.0]))
        .with_min_inner_size([260.0, 170.0]);
    if let Some(pos) = state.window_pos {
        viewport = viewport.with_position(pos);
    }
    if state.pinned {
        viewport = viewport.with_always_on_top();
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "Scratchpad",
        options,
        Box::new(move |cc| {
            let mut app = scrpad::App::new(cc, state, path, startup_error);
            if let scrpad::cli::Command::Note(text) = command {
                app.add_note_with_text(text, 0.0);
            }
            if let Some(listener) = listener {
                app.serve_commands(&cc.egui_ctx, listener);
            }
            Ok(Box::new(app))
        }),
    )
}

/// Prints the answer to a command; the exit code.
fn finish(reply: Reply) -> i32 {
    use std::io::Write;
    match reply {
        Reply::Ok => 0,
        Reply::Text(mut text) => {
            // `scrpad -` drops one trailing newline; `--read` puts it back.
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            let mut out = std::io::stdout().lock();
            match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
                Ok(()) => 0,
                // `scrpad --list | head -1`, say.
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => 0,
                Err(e) => {
                    eprintln!("scrpad: could not write the text: {e}");
                    1
                }
            }
        }
        Reply::Error(message) => {
            eprintln!("scrpad: {message}");
            1
        }
        Reply::Busy => {
            eprintln!("scrpad: another scrpad is busy with the notes file; try again");
            1
        }
    }
}
