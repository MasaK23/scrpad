// A window app on Windows, so that no console opens with it; terminals get `scrpad.com`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    scrpad::launch::run(scrpad::launch::Window::Here)
}
