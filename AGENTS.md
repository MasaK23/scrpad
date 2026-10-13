# scrpad — agent guide

A frameless egui/eframe scratchpad with hidden and encrypted notes. README.md explains the
design; read the section you need, not the whole file.

## Commands

- `just setup` — once: installs the tools pinned in `mise.toml` and the git hook.
- `just check` — the gate. Run before every commit; CI runs the same command.
- `just fmt` — rustfmt + shfmt. `just test` — unit and headless UI tests (images go to `shots/`).

Rust is pinned in `rust-toolchain.toml`. Never weaken a check to make it pass: fix the cause,
or add one narrow suppression with the reason next to it.

## Where things are

| Path | What | Read when |
|---|---|---|
| `src/session.rs` | what the app does, UI-free: notes, autosave, sealing worker, locking, idle | changing behaviour |
| `src/app.rs` | the egui window: drawing, popups, shortcuts, pin, export | GUI layout or window work |
| `src/launch.rs` | what a launch does: GUI, `--tui`, or a script command; `scrpad.com` on Windows | startup work |
| `src/store.rs` | notes model, JSON load/save (atomic, 0600) | touching persistence |
| `src/crypto.rs` | SimpleTextCrypt format (Argon2id + AES-256-GCM) | touching encryption |
| `src/theme.rs`, `src/icons.rs` | design tokens, Lucide icons | visual changes |
| `src/hyprland.rs` | Hyprland IPC for pinning | window-manager work |
| `src/update.rs` | user-started check for a newer GitHub release | anything network-related |
| `src/cli.rs`, `src/instance.rs` | command line, single-instance socket/pipe protocol | launch and socket work |
| `src/scripts.rs` | `--append`/`--list`/`--read`: what scripts and agents may do | anything near script access |
| `src/clipboard.rs` | clears a copied secret if the clipboard still holds it | clipboard work |
| `src/tui/` | terminal frontend (`--tui`): keys and drawing, own editor, terminal loop | terminal UI work |
| `tests/ui.rs` | egui_kittest UI tests | any UI change |
| `tests/tui.rs` | terminal UI tests on ratatui's `TestBackend` | any terminal UI change |
| README.md § Hiding and encryption, § Scripts and agents | the security model and its limits | any change near secrets |

## Invariants

- An encrypted note reaches the disk only as its cipher, never as plaintext — not in
  `notes.json`, not in exports, not in temp files.
- A failed save keeps the changes dirty and visible to the user; it never pretends to have saved.
- An unreadable `notes.json` is moved aside, never overwritten.
- No network access unless the user explicitly asks (`…` → Check for updates…, `src/update.rs`).
  No telemetry, no automatic or start-up checks; tests never call GitHub.
- Scripts (`src/scripts.rs`) only ever append: they never replace or delete anything, and never
  read a hidden or an encrypted note. Reading the others is off unless the user turns it on.
- The crypto format stays compatible with SimpleTextCrypt 5.x (`tests/data/` vectors).
- After visual changes, refresh the README images in `docs/` from `shots/`.
