#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    let path = scrpad::store::State::default_path();
    let state = path.as_deref().map(scrpad::store::State::load).unwrap_or_default();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Scratchpad")
        .with_app_id("scrpad")
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
    eframe::run_native("Scratchpad", options, Box::new(move |cc| Ok(Box::new(scrpad::App::new(cc, state, path)))))
}
