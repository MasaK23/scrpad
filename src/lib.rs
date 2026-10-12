//! Scratchpad: a tiny frameless floating notepad. egui draws every pixel; the OS only gives a window.

mod app;
#[cfg(target_os = "linux")]
pub mod hyprland;
pub mod icons;
pub mod store;
pub mod theme;

pub use app::App;
