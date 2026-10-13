//! `scrpad.com` on Windows: the same program as a console app. cmd and PowerShell run it for a
//! plain `scrpad` (`.com` comes before `.exe`), so commands print there and the terminal waits
//! for their exit code; `scrpad.exe` alone would detach and lose both. It opens no window itself.

fn main() -> eframe::Result {
    scrpad::launch::run(scrpad::launch::Window::Detached)
}
