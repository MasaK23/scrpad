#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Set at runtime because eframe otherwise puts its own egui icon on the window, the Windows
/// taskbar and the macOS Dock and Cmd-Tab switcher, over the one in the .app bundle.
/// macOS gets the icon with the padding of Apple's icon grid, elsewhere it fills its square.
#[cfg(target_os = "macos")]
const ICON: &[u8] = include_bytes!("../assets/app-icon/scrpad.png");
#[cfg(not(target_os = "macos"))]
const ICON: &[u8] = include_bytes!("../assets/app-icon/scrpad-256.png");

fn main() -> eframe::Result {
    // A running scrpad takes over a second launch (and `--toggle` from a compositor key).
    #[cfg(target_os = "linux")]
    let listener = match scrpad::instance::socket_path() {
        Some(socket) => match scrpad::instance::claim(&socket, scrpad::instance::Command::from_args(std::env::args().skip(1))) {
            Ok(scrpad::instance::Claim::Forwarded) => return Ok(()),
            Ok(scrpad::instance::Claim::Listening(listener)) => Some(listener),
            Err(error) => {
                eprintln!("scrpad: {}: {error}", socket.display());
                None
            }
        },
        None => None,
    };
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
            #[allow(unused_mut)]
            let mut app = scrpad::App::new(cc, state, path, startup_error);
            #[cfg(target_os = "linux")]
            if let Some(listener) = listener {
                app.serve_commands(&cc.egui_ctx, listener);
            }
            Ok(Box::new(app))
        }),
    )
}
