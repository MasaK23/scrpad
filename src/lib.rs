//! Scratchpad: a tiny frameless floating notepad. egui draws every pixel; the OS only gives a window.

mod app;
pub mod icons;
pub mod store;
pub mod theme;

pub use app::App;
