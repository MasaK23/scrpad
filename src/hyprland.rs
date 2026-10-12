//! Hyprland: Wayland gives an app no way to stay on top, but Hyprland's IPC socket does.
//! The pin button then does what Omarchy's SUPER+O does (float + pin), and pinning done
//! with the compositor's own keys is reflected back in the button. Hiding moves the window to
//! a special workspace, since Hyprland has no minimize.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

/// Where a hidden window waits: a toggle bound to this special workspace also shows it.
const HIDDEN: &str = "special:hidden";

#[derive(Clone, Debug)]
pub struct Hyprland {
    socket: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowState {
    pub floating: bool,
    pub pinned: bool,
    /// Position and size in Hyprland's logical layout pixels.
    pub at: [i64; 2],
    pub size: [i64; 2],
    /// On a special workspace (hidden, or a scratchpad): its pin and geometry say nothing.
    pub hidden: bool,
}

impl Hyprland {
    /// `Some` when running under Hyprland (its instance signature is in the environment).
    pub fn detect() -> Option<Self> {
        let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        // Newer Hyprland keeps the socket under XDG_RUNTIME_DIR, older ones under /tmp.
        [runtime.map(|r| r.join("hypr")), Some(PathBuf::from("/tmp/hypr"))]
            .into_iter()
            .flatten()
            .map(|dir| dir.join(&sig).join(".socket.sock"))
            .find(|p| p.exists())
            .map(|socket| Self { socket })
    }

    fn request(&self, msg: &str) -> std::io::Result<String> {
        let mut s = UnixStream::connect(&self.socket)?;
        s.set_read_timeout(Some(Duration::from_millis(500)))?;
        s.write_all(msg.as_bytes())?;
        let mut out = String::new();
        s.read_to_string(&mut out)?;
        Ok(out)
    }

    /// This process's window: its address and state.
    fn own_window(&self) -> Option<(String, WindowState)> {
        let json: serde_json::Value = serde_json::from_str(&self.request("j/clients").ok()?).ok()?;
        parse_client(&json, std::process::id())
    }

    fn own_address(&self) -> Option<String> {
        self.own_window().map(|(addr, _)| addr)
    }

    pub fn window_state(&self) -> Option<WindowState> {
        self.own_window().map(|(_, s)| s)
    }

    /// Runs a dispatcher in the Lua syntax (Hyprland 0.55+), falling back to the classic one.
    fn dispatch(&self, lua: &str, classic: &str) -> bool {
        let ok = |r: std::io::Result<String>| r.is_ok_and(|r| r.trim() == "ok");
        ok(self.request(&format!("dispatch {lua}"))) || ok(self.request(&format!("dispatch {classic}")))
    }

    /// Puts the (floating) window back where it was: Wayland gives the app no say in this.
    pub fn place(&self, size: Option<[f32; 2]>, pos: Option<[f32; 2]>) {
        let Some((addr, state)) = self.own_window() else { return };
        if !state.floating {
            return;
        }
        let win = format!("address:{addr}");
        if let Some([w, h]) = size {
            let (w, h) = (w.round() as i64, h.round() as i64);
            self.dispatch(
                &format!(r#"hl.dsp.window.resize({{ x = {w}, y = {h}, window = "{win}" }})"#),
                &format!("resizewindowpixel exact {w} {h},{win}"),
            );
        }
        if let Some([x, y]) = pos {
            let (x, y) = (x.round() as i64, y.round() as i64);
            self.dispatch(
                &format!(r#"hl.dsp.window.move({{ x = {x}, y = {y}, window = "{win}" }})"#),
                &format!("movewindowpixel exact {x} {y},{win}"),
            );
        }
    }

    /// Pinning floats the window first (pin only applies to floating windows); unpinning
    /// leaves it floating. Returns false when the window isn't mapped yet or a call failed.
    pub fn set_pinned(&self, pinned: bool) -> bool {
        let Some((addr, state)) = self.own_window() else { return false };
        let win = format!("address:{addr}");
        if pinned
            && !state.floating
            && !self.dispatch(&format!(r#"hl.dsp.window.float({{ action = "enable", window = "{win}" }})"#), &format!("setfloating {win}"))
        {
            return false;
        }
        if state.pinned == pinned {
            return true;
        }
        let action = if pinned { "enable" } else { "disable" };
        // The classic `pin` dispatcher toggles; the state check above makes that safe.
        self.dispatch(&format!(r#"hl.dsp.window.pin({{ action = "{action}", window = "{win}" }})"#), &format!("pin {win}"))
    }

    /// Moves the window out of sight. A pinned window shows on every workspace and can't be
    /// moved, so it is unpinned first; `show` pins it again.
    pub fn hide(&self) -> bool {
        let Some((addr, state)) = self.own_window() else { return false };
        if state.pinned && !self.set_pinned(false) {
            return false;
        }
        let win = format!("address:{addr}");
        self.dispatch(
            &format!(r#"hl.dsp.window.move({{ workspace = "{HIDDEN}", follow = false, window = "{win}" }})"#),
            &format!("movetoworkspacesilent {HIDDEN},{win}"),
        )
    }

    /// Brings the window to the workspace in front of the user and focuses it.
    pub fn show(&self, pinned: bool) -> bool {
        let Some(addr) = self.own_address() else { return false };
        let win = format!("address:{addr}");
        let workspace = self.request("j/activeworkspace").ok().and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok());
        let Some(id) = workspace.and_then(|w| w["id"].as_i64()) else { return false };
        let moved = self.dispatch(
            &format!(r#"hl.dsp.window.move({{ workspace = {id}, follow = false, window = "{win}" }})"#),
            &format!("movetoworkspacesilent {id},{win}"),
        );
        if pinned {
            self.set_pinned(true);
        }
        moved && self.dispatch(&format!(r#"hl.dsp.focus({{ window = "{win}" }})"#), &format!("focuswindow {win}"))
    }

    /// Whether this process's window has the keyboard focus.
    pub fn is_focused(&self) -> bool {
        let active = self.request("j/activewindow").ok().and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok());
        active.as_ref().and_then(|w| w["address"].as_str()).is_some_and(|a| Some(a) == self.own_address().as_deref())
    }
}

/// The client of process `pid` in a `j/clients` reply.
fn parse_client(clients: &serde_json::Value, pid: u32) -> Option<(String, WindowState)> {
    let c = clients.as_array()?.iter().find(|c| c["pid"].as_u64() == Some(u64::from(pid)))?;
    let pair = |v: &serde_json::Value| Some([v[0].as_i64()?, v[1].as_i64()?]);
    let state = WindowState {
        floating: c["floating"].as_bool()?,
        pinned: c["pinned"].as_bool()?,
        at: pair(&c["at"])?,
        size: pair(&c["size"])?,
        hidden: c["workspace"]["name"].as_str().is_some_and(|n| n.starts_with("special:")),
    };
    Some((c["address"].as_str()?.to_owned(), state))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clients(workspace: &str) -> serde_json::Value {
        serde_json::json!([
            { "pid": 1, "address": "0xa", "floating": false, "pinned": false, "at": [0, 0], "size": [9, 9],
              "workspace": { "id": 1, "name": "1" } },
            { "pid": 42, "address": "0xb", "floating": true, "pinned": true, "at": [10, 20], "size": [380, 300],
              "workspace": { "id": -98, "name": workspace } },
        ])
    }

    #[test]
    fn finds_own_window_by_pid() {
        let (addr, state) = parse_client(&clients("4"), 42).unwrap();
        assert_eq!(addr, "0xb");
        assert_eq!(state, WindowState { floating: true, pinned: true, at: [10, 20], size: [380, 300], hidden: false });
        assert!(parse_client(&clients("4"), 7).is_none());
    }

    #[test]
    fn a_special_workspace_counts_as_hidden() {
        assert!(parse_client(&clients(HIDDEN), 42).unwrap().1.hidden);
        assert!(parse_client(&clients("special:scratchpad"), 42).unwrap().1.hidden);
    }
}
