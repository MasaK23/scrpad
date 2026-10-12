//! Headless UI tests with egui_kittest: drive the real `App::draw` and render it with wgpu.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use scrpad::App;
use scrpad::store::{Note, State};
use std::sync::{Mutex, MutexGuard};

// Multiple concurrent wgpu harnesses can crash the software Vulkan backend.
static UI_TEST_LOCK: Mutex<()> = Mutex::new(());

fn ui_test_guard() -> MutexGuard<'static, ()> {
    UI_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn sample() -> State {
    let mut s = State { notes: vec![Note::new("Scratchpad 1".into()), Note::new("Scratchpad 2".into())], active: 1, ..State::default() };
    s.notes[0].text = "TODO: ship it".into();
    s
}

fn harness(state: State) -> Harness<'static, App> {
    harness_at(state, None)
}

/// With a path the app autosaves to it, like the real one does.
fn harness_at(state: State, path: Option<std::path::PathBuf>) -> Harness<'static, App> {
    let mut app = None;
    let mut h = Harness::builder().with_size(egui::vec2(380.0, 300.0)).with_pixels_per_point(2.0).build_ui_state(
        move |ui, app_slot: &mut App| app_slot.draw(ui),
        {
            let ctx = egui::Context::default();
            app.replace(App::with_state(&ctx, state, path));
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
    let _guard = ui_test_guard();
    let mut h = harness(sample());
    shot(&mut h, "dark");
    h.state_mut().state.theme = scrpad::theme::ThemeKind::Light;
    h.run_steps(3);
    shot(&mut h, "light");
}

#[test]
fn add_hide_and_close_notes() {
    let _guard = ui_test_guard();
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
    let _guard = ui_test_guard();
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

/// Steps the UI until `done` holds, giving the Argon2id workers real time to finish.
fn run_until(h: &mut Harness<'_, App>, done: impl Fn(&App) -> bool) {
    for _ in 0..200 {
        h.run_steps(1);
        if done(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("condition not reached");
}

#[test]
fn encrypted_notes_reach_the_disk_only_encrypted() {
    let _guard = ui_test_guard();
    let dir = std::env::temp_dir().join(format!("scratchpad-ui-crypt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("notes.json");
    let mut state = sample();
    state.active = 0;
    state.notes[0].text = "sk-live-SECRET".into();
    let mut h = harness_at(state, Some(path.clone()));
    let on_disk = || std::fs::read_to_string(&path).unwrap_or_default();

    // Encrypt through the popup: the password twice.
    h.get_by_label("Encryption").click();
    h.run_steps(2);
    h.get_by_label("Password").type_text("hunter2");
    h.get_by_label("Repeat password").click();
    h.run_steps(1);
    h.get_by_label("Repeat password").type_text("hunter2");
    h.run_steps(1);
    shot(&mut h, "encrypt-popup");
    h.get_by_label("Encrypt").click();
    run_until(&mut h, |a| a.state.notes[0].is_encrypted());
    assert!(on_disk().contains("\"cipher\""));
    assert!(!on_disk().contains("SECRET"));
    h.run_steps(2);
    shot(&mut h, "encrypted");

    // Typing on re-seals in the background; the plaintext never shows up in the file.
    h.state_mut().state.notes[0].text.push_str(" and MORE");
    h.state_mut().save_now();
    let cipher = h.state().state.notes[0].cipher.clone().unwrap();
    assert_eq!(scrpad::crypto::decrypt("hunter2", &cipher).unwrap(), "sk-live-SECRET and MORE");
    assert!(!on_disk().contains("MORE"));

    // Lock: the plaintext leaves memory; the editor asks for the password.
    h.get_by_label("Encryption").click();
    h.run_steps(2);
    assert!(h.state().state.notes[0].is_locked());
    assert_eq!(h.state().state.notes[0].text, "");
    shot(&mut h, "locked");

    h.get_by_label("Password").click();
    h.run_steps(1);
    h.get_by_label("Password").type_text("wrong");
    h.get_by_label("Unlock").click();
    run_until(&mut h, |a| a.state.notes[0].password.is_none());
    h.run_steps(2);
    assert!(h.state().state.notes[0].is_locked());
    shot(&mut h, "wrong-password");
    h.get_by_label("Password").type_text("hunter2");
    h.get_by_label("Unlock").click();
    run_until(&mut h, |a| !a.state.notes[0].is_locked());
    assert_eq!(h.state().state.notes[0].text, "sk-live-SECRET and MORE");

    // A restart comes up locked and decrypts to the same text.
    drop(h);
    let mut loaded = State::load(&path).unwrap();
    assert!(loaded.notes[0].is_locked());
    loaded.notes[0].unlock("hunter2").unwrap();
    assert_eq!(loaded.notes[0].text, "sk-live-SECRET and MORE");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn edits_are_resealed_by_autosave() {
    let _guard = ui_test_guard();
    let dir = std::env::temp_dir().join(format!("scratchpad-ui-reseal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("notes.json");
    let mut h = harness_at(sample(), Some(path.clone()));
    h.state_mut().encrypt_note("pw", 0.0).unwrap();
    h.run_steps(2);
    // The editor has focus again after encrypting: type into it like a user.
    h.event(egui::Event::Text("typed later".into()));
    let sealed = |a: &App| {
        let cipher = std::fs::read_to_string(dir.join("notes.json")).ok().and_then(|f| {
            let s: serde_json::Value = serde_json::from_str(&f).ok()?;
            s["notes"][1]["cipher"].as_str().map(str::to_owned)
        });
        a.state.notes[1].text == "typed later" && cipher.is_some_and(|c| scrpad::crypto::decrypt("pw", &c).as_deref() == Ok("typed later"))
    };
    run_until(&mut h, sealed);
    assert!(!std::fs::read_to_string(&path).unwrap().contains("typed later"));
    drop(h);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn encrypt_popup_never_targets_another_note() {
    let _guard = ui_test_guard();
    let mut state = sample();
    state.notes[1].encrypt("old").unwrap();
    state.notes[1].lock();
    state.active = 0;
    let mut h = harness(state);
    h.get_by_label("Encryption").click();
    h.run_steps(2);
    // Switching notes with the popup open closes it: it was opened for note 0, not the locked one.
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::Tab);
    h.run_steps(2);
    assert!(h.query_by_label("Repeat password").is_none());
    let locked = &h.state().state.notes[1];
    assert!(locked.is_locked());
    assert_eq!(scrpad::crypto::decrypt("old", locked.cipher.as_ref().unwrap()).unwrap(), "");
    assert!(h.state_mut().encrypt_note("new", 0.0).is_err());
}

#[test]
fn enter_moves_to_the_repeat_field_then_encrypts() {
    let _guard = ui_test_guard();
    let mut h = harness(sample());
    h.get_by_label("Encryption").click();
    h.run_steps(2);
    h.get_by_label("Password").type_text("pw");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    h.get_by_label("Repeat password").type_text("pw");
    h.key_press(egui::Key::Enter);
    run_until(&mut h, |a| a.state.current().is_encrypted());
    assert_eq!(h.state().state.current().password.as_deref(), Some("pw"));
}

#[test]
fn ids_of_closed_notes_are_never_reused() {
    let mut s = sample();
    let last = s.notes[1].id;
    s.close_note(1);
    s.add_note();
    assert!(s.current().id > last);
    // Closing the only note: its placeholder gets a fresh id too.
    let mut s = State::default();
    let only = s.notes[0].id;
    s.close_note(0);
    assert!(s.notes[0].id > only);
}

#[test]
fn shows_the_encrypted_text_on_request() {
    let _guard = ui_test_guard();
    let mut state = sample();
    state.active = 0;
    let mut h = harness(state);
    h.state_mut().encrypt_note("pw", 0.0).unwrap();
    h.run_steps(2);
    // Edited after encrypting: the view seals first, so it shows what the text is now.
    h.state_mut().state.notes[0].text.push_str(" today");
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::E);
    h.run_steps(2);
    assert!(h.state().showing_cipher());
    let cipher = h.state().state.notes[0].cipher.clone().unwrap();
    assert_eq!(scrpad::crypto::decrypt("pw", &cipher).unwrap(), "TODO: ship it today");
    h.get_by_label("Encrypted text");
    shot(&mut h, "cipher-view");
    // Escape goes back to the text; a locked note offers the view too.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!h.state().showing_cipher());
    h.state_mut().lock_note(0.0);
    h.run_steps(2);
    h.get_by_label("Show encrypted text").click();
    h.run_steps(2);
    assert!(h.state().showing_cipher());
    shot(&mut h, "cipher-view-locked");
}
