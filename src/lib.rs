//! Scratchpad: a tiny frameless floating notepad. egui draws every pixel; the OS only gives a window.

mod app;
pub mod cli;
pub mod clipboard;
pub mod crypto;
#[cfg(target_os = "linux")]
pub mod hyprland;
pub mod icons;
pub mod instance;
pub mod scripts;
pub mod session;
pub mod store;
pub mod theme;
pub mod update;

pub use app::App;
pub use session::Session;
