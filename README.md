# scrpad

[![CI](https://github.com/mhavo/scrpad/actions/workflows/ci.yml/badge.svg)](https://github.com/mhavo/scrpad/actions/workflows/ci.yml)

A tiny scratchpad: a small, frameless, floating notepad. The window holds only the essentials: multiple notes in tabs, hiding a note from prying eyes (API keys and the like), password encryption, copy to clipboard, save to file, and an "always on top" pin. Everything is saved automatically.

![Scratchpad](docs/screenshot.png)

| Light theme | Encrypting a note | Locked note |
|---|---|---|
| ![Light theme](docs/light.png) | ![Encrypt popup](docs/encrypt.png) | ![Locked note](docs/locked.png) |

## Download

Prebuilt binaries for Linux (x86_64, aarch64), macOS (universal app for Apple Silicon and Intel) and Windows (x86_64) are on the [Releases](https://github.com/mhavo/scrpad/releases) page. Each archive holds a single executable (on macOS, an app bundle); there is nothing to install. `SHA256SUMS` lists the checksums.

- **Linux:** `tar xzf scrpad-*.tar.gz`, then either run `scrpad` from the folder or run `./install.sh`, which installs the binary, a menu entry and the icon into `~/.local` (set `PREFIX` for another place; `./install.sh --uninstall` removes them). On Wayland, the window and taskbar icon only appear once the menu entry is installed, since the compositor looks the icon up by the app id `scrpad`. Needs glibc 2.35 or newer and working Vulkan or OpenGL drivers.
- **macOS:** `scrpad-*-macos-universal.zip` holds `scrpad.app`, which runs natively on both Apple Silicon and Intel Macs (macOS 10.13 or newer). Drag it to Applications. The app is not notarized, since that needs a paid Apple Developer account, so Gatekeeper blocks the first start of a downloaded copy. Either right-click the app, choose Open and confirm, or clear the quarantine flag once with `xattr -dr com.apple.quarantine /Applications/scrpad.app`.
- **Windows:** extract the zip and run `scrpad.exe`. SmartScreen may warn about an unknown publisher, since the binary is not signed.

## Build

Requires stable Rust 1.85 or newer (the crate uses edition 2024).

```
cargo build --release      # binary at target/release/scrpad
cargo run --release        # or build and run in one go
cargo build --profile dist # smallest binary for distribution, at target/dist/scrpad
```

The `release` profile skips link-time optimization so rebuilds take seconds. `dist` adds full LTO and a single codegen unit: a binary about a third smaller, at the cost of a final link that takes a few minutes.

On Linux, rendering goes through wgpu, so you need working Vulkan or OpenGL drivers.

## Usage

| Action | Mouse | Keyboard |
|---|---|---|
| New note | `+`, or double-click empty space in the tab bar | Ctrl+N / Ctrl+T |
| Close note (Undo from the toast) | tab `×` or middle click | Ctrl+W |
| Rename | double-click the tab, or `…` → Rename note | F2 |
| Switch note | click | Ctrl+Tab / Ctrl+Shift+Tab |
| Reorder notes | drag a tab sideways | |
| Hide/show content | eye icon | Ctrl+Shift+H |
| Encrypt / lock note | lock icon | Ctrl+Shift+L |
| Lock all encrypted notes | `…` → Lock all | Ctrl+Shift+K |
| Lock and hide again when stepping away (both on by default, below) | `…` → Lock encrypted notes automatically, Hide notes again automatically | |
| Show the encrypted text (Esc to go back) | `…` → Show encrypted text | Ctrl+Shift+E |
| Copy all | copy icon | Ctrl+Shift+C |
| Save to file | download icon | Ctrl+S |
| Clear (Undo from the toast) | trash icon | |
| Font (Inter / JetBrains Mono, size) | `Aa` | |
| Theme (dark/light) | `…` | |
| Float on top | pin | |
| Hide window (show it again: `scrpad --toggle`, below) | `…` → Hide window | Ctrl+H |
| Check for updates | `…` → Check for updates… | |
| Move / resize window | drag the title bar / the edges | |

On macOS, Ctrl means Cmd, except for Ctrl+Tab and Ctrl+Shift+Tab.

From the command line (`scrpad --help`):

```
scrpad              show the window
scrpad --toggle     hide the window when it has the focus, show it otherwise
scrpad --new        open a new empty note
scrpad -            open a new note with the text from stdin
```

`scrpad -` takes at most 1 MiB of UTF-8 text and drops the one trailing newline `echo` adds; anything else is refused with a message on stderr. The note is a plain note titled like any new one: hide or encrypt it as needed. On Linux a running scrpad takes over (below), so the note opens in the window you already have. For example, to turn the clipboard into a note with a key in Hyprland:

```
bind = SUPER SHIFT, V, exec, wl-paste | scrpad -
```

Notes are stored in `~/.config/scrpad/notes.json` (macOS: `~/Library/Application Support/scrpad/`, Windows: `%APPDATA%\scrpad\`). If `notes.json` exists but cannot be read, it is moved aside to `notes.json.bak` (or `.bak.1`, `.bak.2`, …) instead of being overwritten.

**No network unless you ask.** scrpad never connects anywhere on its own: there is no telemetry and no automatic update check. `…` → *Check for updates…* makes one HTTPS request to the GitHub API for the latest release (GitHub sees your IP address and the scrpad version). If a newer version exists, *Download* opens its release page in the browser; nothing is downloaded or installed by the app.

## Hiding and encryption

**Hiding** (eye icon) only masks the content on screen, against screenshots and shoulder surfing. A hidden note is still stored as plain text.

**Encryption** (lock icon, next to the eye) protects the note on disk:

- Click the lock on an unencrypted note and set a password (entered twice). The note is now encrypted.
- An encrypted note's text is only ever written to disk encrypted. The plaintext is kept only in memory, passwords are never stored, and locking a note drops its text, password and editor undo history.
- At start-up, encrypted notes are locked; the editor shows a password field to unlock them. Note titles are **not** encrypted.
- On an unlocked encrypted note, click the lock (or press Ctrl+Shift+L) to lock it right away. To decrypt it permanently, use `…` → Remove encryption.
- `…` → Show encrypted text (Ctrl+Shift+E) shows the note exactly as it is saved, as a read-only SimpleTextCrypt message, even while the note is locked. The copy button then copies the encrypted message.
- Save to file on an encrypted note writes the encrypted message, not the plaintext. Copy to clipboard copies the plaintext, since it is already in memory and you asked for it explicitly (and clears it again, below).

Hiding and encryption are independent. Use both for secrets that should neither be stored in plain text nor be visible on screen.

**Clipboard.** Copying a hidden or encrypted note (copy icon, Ctrl+Shift+C) clears the clipboard 30 seconds later, and on quit, but only if it still holds exactly that text: anything you copied since is left alone. Plain notes are never cleared. scrpad compares a hash of the copied text, not a kept copy. It needs to read the clipboard for that, which works on X11, macOS, Windows and Wayland compositors with the data-control protocol (Hyprland, Sway and other wlroots compositors, KDE). Where the clipboard cannot be read (GNOME on Wayland, for one), nothing is cleared rather than clearing blindly, and the toast says so. A clipboard manager or history (cliphist, Klipper, Windows clipboard history) may keep its own copy, which scrpad does not touch.

**Stepping away.** Two options in the `…` menu, both on by default, act when the window is hidden (Ctrl+H, `scrpad --toggle`) and after 5 minutes without keyboard or mouse input:

- *Lock encrypted notes automatically* locks every unlocked encrypted note, exactly like the lock button: what was typed is encrypted and saved first, then the text, password and undo history leave memory. `…` → Lock all (Ctrl+Shift+K) does the same right away, whatever the option says.
- *Hide notes again automatically* hides again the hidden notes you revealed with the eye, so revealing one is only for the moment. Notes you never hid stay as they are.

The idle timer runs while the window is drawn. On Wayland a window that is not visible (on another workspace, say) may get no frames; the idle lock then happens at the latest on the first frame it draws again, before any text is shown.

**Format.** Compatible with SimpleTextCrypt 5.x: the key is derived from the password with Argon2id (t=5, m=64 MiB, p=2) using the random 12-byte IV as the salt, and the text is encrypted with AES-256-GCM. The text form is Base64(IV) followed by Base64(ciphertext‖tag), wrapped at 76 columns. Messages can be decrypted with other SimpleTextCrypt-compatible tools.

**Limits.** Encryption protects data at rest. Use a long, unique password: an attacker who obtains `notes.json` can try password guesses offline, and a forgotten password cannot be recovered. SimpleTextCrypt does not bind a ciphertext to a particular note or revision (there is no authenticated context/AAD or rollback protection). Someone who can modify `notes.json` can therefore swap valid encrypted messages between notes or replay an older valid message; this does not break AES-GCM or reveal the plaintext.

Encryption does not protect against malware running in your session while a note is unlocked or plaintext that the OS may have written to swap. Memory is not wiped securely, so copies of the plaintext may linger in freed memory until it is reused. Text typed *before* a note was encrypted may already have been autosaved as plain text; the encrypted file replaces it, but the old blocks are not securely erased from the disk. For real secrets, encrypt the note first and type afterwards.

## Why Rust, and how

**Why.** A notepad has to start instantly, stay light and never crash. Rust compiles to a single native binary with no runtime, garbage collector or browser engine; an Electron version would ship all of Chromium. The same code builds for Linux, macOS, Windows and, if needed, WASM. The type system and ownership rules catch typical bugs at compile time, such as data races on state shared between the save thread and the UI.

**How.** The app owns every pixel and asks the operating system for nothing but a window:

1. **egui draws everything.** egui is an *immediate mode* GUI: `App::draw` describes the whole UI on every frame. eframe and wgpu push the triangles to the GPU (Metal, DX12/Vulkan, Vulkan/GL), and winit provides just the window and input. The window is created with `with_decorations(false)` and `with_transparent(true)`; rounded corners, shadows and buttons are all drawn by the app.
2. **Its own design system.** `src/theme.rs`: every colour and radius lives in a `Tokens` struct (dark and light theme). The fonts (Inter, JetBrains Mono) are embedded in the binary. `src/icons.rs`: Lucide SVGs are rasterised at exactly the needed size and tinted with the theme colour. No Unicode glyphs are used as icons.
3. **Platform differences stay at the edges.** The clipboard is written through eframe (`ctx.copy_text`) and read back only to clear a copied secret (`src/clipboard.rs`: arboard, and data-control on Wayland), and the native save dialog uses rfd on its own thread so the window keeps redrawing while the dialog is open. Moving and resizing use `ViewportCommand::StartDrag` and `BeginResize`, since a frameless window has no system title bar to drag.
4. **The result is checked by looking at it.** `tests/ui.rs` drives the real UI headlessly with egui_kittest. The tests click buttons by their accesskit names and render images into `shots/` with wgpu. The same accesskit names are exposed to screen readers.

The trade-offs: the app does not look native, and the binary is about 15 MB (`dist` profile) because of wgpu. A smaller one is possible with eframe's `glow` backend.

## Wayland / Hyprland

On Wayland, an app cannot make its own window "always on top" or choose its position; the compositor decides. The pin works on X11, macOS and Windows through winit's `WindowLevel`.

**Hyprland** is detected from the `HYPRLAND_INSTANCE_SIGNATURE` variable, and through its IPC socket the app does the same as Omarchy's SUPER+O:

- The pin makes the window floating and pinned. Unpinning removes the pin but leaves the window floating.
- When the window is pinned or unpinned with Hyprland's own keys (SUPER+O or SUPER+T), the pin button catches up within about a second.
- The size and position of a floating window are remembered and restored at start-up. The size of a tiled window is not saved.
- Ctrl+H moves the window to the special workspace `special:hidden` (Hyprland has no minimize). A pinned window is unpinned to move and pinned again when it comes back.
- Commands are sent first in the new Lua syntax (`hl.dsp.window.pin({ action = "enable", … })`, Hyprland 0.55+), falling back to the old `pin`/`setfloating` syntax.

Elsewhere, the size and, where the platform reports it, the position are read from winit.

**Show and hide with one key.** Wayland gives apps no global shortcuts, so the key belongs to the compositor. On Linux, scrpad runs once per session: launching it again hands over to the running window through a socket in `$XDG_RUNTIME_DIR` (local, never the network). `scrpad` brings the window to the current workspace and focuses it; `scrpad --toggle` hides it when it has the focus and shows it otherwise; `scrpad --new` and `scrpad -` add a note and bring the window here. Bind that command to a key, for example `bind = SUPER, H, exec, scrpad --toggle` in `hyprland.conf`, or with Omarchy's keybindings editor. A hidden window draws nothing until it is shown, so hiding saves pending changes first. Outside Hyprland, hiding minimizes the window.

## Tests and checks

```
just setup            # once: the tools pinned in mise.toml and the git pre-commit hook
just check            # everything CI gates on: rustfmt, clippy, typos, shellcheck, cargo-deny, workflow lint, tests
cargo test            # just the unit tests + headless UI tests, images go to shots/
```

Rust is pinned in `rust-toolchain.toml`; the other tools (just, cargo-deny, typos, shfmt, shellcheck, actionlint, zizmor, gitleaks) in `mise.toml`. The pre-commit hook runs the fast subset (`just pre-commit`: formatting, typos, gitleaks on the staged changes).

The README images in `docs/` are copies of `shots/dark.png`, `light.png`, `encrypt-popup.png` and `locked.png`. Refresh them after visual changes.

## Releasing

CI (`.github/workflows/ci.yml`) runs `just check` and a gitleaks scan of the history on Linux and checks that the app builds on macOS and Windows. To publish a release, bump `version` in `Cargo.toml`, commit, and push a matching tag:

```
git tag v0.1.0
git push origin v0.1.0
```

`.github/workflows/release.yml` builds the `dist` profile for every platform and creates a GitHub release with the archives, `SHA256SUMS` and generated notes. The release is refused if the tag does not match the version in `Cargo.toml`.

On macOS, `packaging/macos/bundle.sh` turns the Apple Silicon and Intel builds into one universal `scrpad.app`: it joins the binaries with `lipo`, fills in `packaging/macos/Info.plist`, builds the icon with `iconutil` and signs the bundle ad hoc. The same script runs locally on a Mac; its header lists the build commands. A bare executable would open in Terminal when started from Finder, which is why macOS gets an app bundle.

The app icon is drawn in `assets/app-icon/scrpad.svg`. After editing it, run `assets/app-icon/render.sh` (needs inkscape and python3) and commit what it renders: the macOS icon (`scrpad.png`), a square crop for Linux (`scrpad-square.svg`, `scrpad-256.png`) and the Windows icon (`scrpad.ico`). The icon reaches each platform in its own way:

- **At runtime, everywhere:** `src/main.rs` sets the icon on the window. Without it, eframe would show its own egui icon, which on macOS also covers the bundle icon in the Dock and the Cmd-Tab switcher while the app runs.
- **Windows:** `build.rs` embeds `scrpad.ico` into `scrpad.exe` with winresource, so Explorer and shortcuts show it.
- **macOS:** `bundle.sh` builds `scrpad.icns` into the app bundle for Finder and the Dock.
- **Linux:** the release archive carries `scrpad.desktop` and the icons from `packaging/linux/`, which `install.sh` puts into the hicolor theme.

## License

The code is licensed under the MIT License, see [LICENSE](LICENSE).

Bundled assets keep their own licenses:

- Fonts: Inter and JetBrains Mono, SIL Open Font License 1.1 (`assets/fonts/OFL-*.txt`).
- Icons: Lucide, ISC License (`assets/icons/LICENSE-lucide.txt`).
