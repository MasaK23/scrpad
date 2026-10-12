//! Headless UI tests with egui_kittest: drive the real `App::draw` and render it with wgpu.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use scratchpad::App;
use scratchpad::store::{Note, State};

fn sample() -> State {
    let mut s = State { notes: vec![Note::new("Scratchpad 1".into()), Note::new("Scratchpad 2".into())], active: 1, ..State::default() };
    s.notes[0].text = "TODO: ship it".into();
    s
}

fn harness(state: State) -> Harness<'static, App> {
    let mut app = None;
    let mut h = Harness::builder().with_size(egui::vec2(380.0, 300.0)).with_pixels_per_point(2.0).build_ui_state(
        move |ui, app_slot: &mut App| app_slot.draw(ui),
        {
            let ctx = egui::Context::default();
            app.replace(App::with_state(&ctx, state, None));
            app.take().unwrap()
        },
    );
    h.run_steps(3);
    h
}

fn shot(h: &mut Harness<'_, App>, name: &str) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shots");
    std::fs::create_dir_all(&dir).unwrap();
    h.render().unwrap().save(dir.join(format!("{name}.png"))).unwrap();
}

#[test]
fn looks_like_the_reference() {
    let mut h = harness(sample());
    shot(&mut h, "dark");
    h.state_mut().state.theme = scratchpad::theme::ThemeKind::Light;
    h.run_steps(3);
    shot(&mut h, "light");
}

#[test]
fn add_hide_and_close_notes() {
    let mut h = harness(sample());
    h.get_by_label("New note").click();
    h.run_steps(2);
    assert_eq!(h.state().state.notes.len(), 3);
    assert_eq!(h.state().state.active, 2);
    // Three tabs overflow the strip, which scrolls to the active one: cycle with Ctrl+Tab.
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Tab);
    h.run_steps(2);
    assert_eq!(h.state().state.active, 0);
    h.get_by_label("Hide note").click();
    h.run_steps(3);
    assert!(h.state().state.notes[0].hidden);
    shot(&mut h, "hidden");
    h.get_by_label("Close note").click();
    h.run_steps(2);
    assert_eq!(h.state().state.notes.len(), 2);
    assert!(h.state().state.notes.iter().all(|n| n.title != "Scratchpad 1"));
    // The toast offers to bring it back.
    h.get_by_label("Undo").click();
    h.run_steps(2);
    assert_eq!(h.state().state.notes[0].text, "TODO: ship it");
}

#[test]
fn undoing_a_clear_never_overwrites_another_note() {
    let mut h = harness(sample());
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Tab);
    h.run_steps(2);
    assert_eq!(h.state().state.active, 0);
    h.get_by_label("Clear note").click();
    h.run_steps(2);
    assert_eq!(h.state().state.notes[0].text, "");
    // The cleared (now empty) note is closed; another note takes its index.
    h.get_by_label("Close note").click();
    h.run_steps(2);
    h.state_mut().state.notes[0].text = "other".into();
    h.get_by_label("Undo").click();
    h.run_steps(2);
    let s = &h.state().state;
    assert_eq!(s.notes[0].text, "other");
    assert_eq!(s.current().text, "TODO: ship it");
}
