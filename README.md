# radial-paste

Hold a hotkey, flick the mouse toward a snippet, let go, and its text is pasted into the app you were typing in. Or tap the hotkey and click a snippet instead. A local settings window edits the snippets and the hotkey.

Windows 10 and 11 only.

## Status

Work in progress, milestones M1 and M2 done. The model, the menu's state machine, the TOML settings file, the settings window, the global hotkey, the input loop and the radial overlay work and are tested. The paste adapter is still a stub that logs what it would do; its module doc lists the Win32 calls that will replace it.

## Scope

**v1**
- One process with a tray icon (Settings / Pause / Quit), a single instance, and optional autostart.
- A global hotkey that can be changed while the app runs. Hold it and flick toward a wedge, then let go, to paste. Tap it to keep the menu open for a click. Esc, a click in the centre, a click off the menu, or the hotkey again closes the menu.
- A radial overlay at the cursor with up to 8 wedges. Each snippet has a short label (at most 24 characters) and its full text.
- Pasting goes through the clipboard and Ctrl+V. The clipboard's contents are restored afterwards, and the pasted text is kept out of Win+V history.
- Settings live in `%APPDATA%\radial-paste\config.toml`. The file is written atomically and has a version field.

**Later**
- More than 8 snippets, as a second ring or pages.
- Typing the text with `SendInput` as a second way to paste.
- Placeholders such as `{date}` and `{clipboard}`.
- Opening the menu with a mouse button.

**Out of scope:** any server or sync, other operating systems, nested menus, rich text.

## Architecture

Hexagonal, as a Cargo workspace:

| Crate | Role |
|---|---|
| `crates/domain` (`radial-domain`) | Pure model: `Snippet`, `Config`, `HotkeyBinding`, `Layout` (which wedge an offset points into). No I/O. |
| `crates/app` (`radial-app`) | Use cases and ports. `Core` is the menu's state machine; `save_config` validates and saves. The ports are `ConfigRepository`, `MenuPresenter`, `TextInjector` and `HotkeyRegistrar`. |
| `crates/store` (`radial-store`) | `ConfigRepository` stored as TOML. |
| `crates/win` (`radial-win`) | Windows adapters and the input thread. |
| `crates/settings` (`radial-settings`) | The egui settings window. |
| `crates/desktop` (`radial-paste`) | The binary and composition root. |

The input thread owns the hotkey, the overlay and the `Core`. It feeds the core `Event`s: `HotkeyPressed`, and `Tick`s that sample the cursor and the keys every 10 ms while the menu is up. The settings window runs on the main thread and sends `ConfigChanged` over a channel.

Key decisions:
- **The key is polled, not hooked.** `RegisterHotKey` reports the key going down. While the menu is up, `GetAsyncKeyState` polling detects the release. There is no `WH_KEYBOARD_LL` hook, because antivirus software flags hooks as keylogger behaviour.
- **The overlay never takes focus** (`WS_EX_NOACTIVATE`), so the target app keeps it and there is nothing to restore. Esc is registered as a temporary hotkey while the menu is up.
- **Paste waits for the modifiers to be released** (up to `modifier_release_timeout_ms`), so Ctrl+V is not received as Ctrl+Shift+V.
- **The Win key cannot be part of the hotkey**, and keys that type text need Ctrl or Alt. The default is `Ctrl+Shift+Space`; plain `Ctrl+Space` is taken by IDEs and input methods.
- **`serde` derives live in the domain**, to avoid duplicating every type as a DTO. The file format (TOML) is chosen only in `store`.

## Known issues to design for

- Windows (UIPI) silently drops input sent from a normal process to an elevated window.
- Pasting into terminals needs Ctrl+Shift+V.
- Releasing an Alt in the hotkey can open the target app's menu bar.
- The clipboard restore races the target app's read of the clipboard.
- Security software may flag `SendInput`. Sign the release build.

## Development

Prerequisites: Rust stable (edition 2024).

```powershell
cargo run -p radial-paste                 # settings window; the paste stub logs to the console
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Set `RUST_LOG=debug` for more output.
