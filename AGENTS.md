# Agent Guide

Read `README.md` first. It has the scope, the architecture and the key decisions. Then read `plan.md`, which has the current state, the open decisions and the milestones. Keep `plan.md` up to date when a milestone or a decision changes.

## Architecture rules

- Hexagonal. `radial-domain` depends on nothing but `serde` and `thiserror`. `radial-app` depends only on the domain. Adapters (`store`, `win`, `settings`) implement the traits in `radial_app::ports` or send `radial_app::Event`s. Only `desktop` wires them together.
- No Windows APIs, file I/O, threads or clocks in `domain` or `app`. Time comes in on events (`Tick { now, .. }`), so the core can be tested with plain values.
- Behaviour belongs in `Core` and is tested there with fakes. Adapters translate; they do not decide.
- The input thread owns the hotkey, the overlay and the `Core`. Other threads talk to it only through the event channel.
- No low-level keyboard or mouse hooks (`WH_KEYBOARD_LL`, `WH_MOUSE_LL`) without asking first. The release is detected by polling.
- The overlay must never take focus.
- Keep `MAX_SNIPPETS` at 8 until the second ring or pages are designed. Ask before adding nested menus.
- Windows only. Do not add other platforms without asking.

## Code style

- `cargo fmt`. Keep `cargo clippy --workspace --all-targets -- -D warnings` clean.
- No `unwrap()` or `expect()` outside tests; return typed errors (`thiserror`).
- Every `unsafe` block gets a comment explaining why it is sound. `domain`, `app`, `store` and `settings` forbid `unsafe`.
- Conventional Commits scoped by crate: `feat(app):`, `fix(win):`.
- Do not commit or push unless asked.

## Checks before finishing

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`

## Environment

Windows 11 with PowerShell.
