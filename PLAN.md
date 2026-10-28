# radial-paste: plan

How to resume work in a new thread: read `README.md` (scope and decisions), `AGENTS.md` (rules), then this file. The **Next step** section says where to start.

---

## 1. Product in one paragraph

A Windows tray app. You hold a global hotkey and a ring of up to 8 text snippets appears around the mouse. Flick toward one and let go, and its text is pasted into the app you were typing in. If you tap the hotkey instead, the ring stays open and you click a snippet. A local settings window edits the snippets, the hotkey and the ring's size. There is no server, no sync, and no other OS.

## 2. Decisions made (with the user)

| Topic | Decision |
|---|---|
| Admin panel | Local settings window only. No server or multiple users. |
| Platform | Windows 10/11 only. |
| Trigger | Hold, flick and release, with tap-then-click from the same code path. A tap released in the dead zone leaves the menu open. |
| Key release detection | `RegisterHotKey` for the press. Then poll `GetAsyncKeyState` every 10 ms while the menu is up. **No low-level hooks**, because antivirus software flags them as keylogger behaviour. |
| Focus | The overlay is `WS_EX_NOACTIVATE`. The target app keeps focus, so nothing needs restoring. |
| Paste | Clipboard + Ctrl+V, then restore the clipboard and keep the text out of Win+V history. The user doesn't mind clobbering the clipboard for now; restoring it is a nicety. |
| Snippets | Up to 8 in one ring (`MAX_SNIPPETS`). More later as a second ring or pages, never nested menus. |
| Labels | Separate from the text, at most 24 characters (`MAX_LABEL_CHARS`). |
| Hotkey rules | The Win key isn't allowed. Keys that type text (letters, digits, Space, `` ` ``) need Ctrl or Alt. F1–F24 may stand alone. Default is `Ctrl+Shift+Space`, since `Ctrl+Space` is taken by IDEs and input methods. |
| Wait before paste | After a pick, wait until the hotkey's modifiers are up (`modifier_release_timeout_ms`, default 300), so Ctrl+V doesn't arrive as Ctrl+Shift+V. |
| Architecture | Hexagonal Cargo workspace (see §4). `serde` derives live in the domain; only `store` chooses TOML. |
| Settings file | `%APPDATA%\radial-paste\config.toml`, written atomically (temp file + rename), with a `version` field (currently 1). |
| Settings UI | egui (eframe). |
| Overlay drawing | Raw Win32 layered window plus `tiny-skia`, not egui. |

## 3. Current state

M1 and M2 are done: the hotkey, the input loop and the overlay are real. Nothing is committed: the repo is on `main`, freshly `git init`'d, and every file is untracked.

Verified on 2026-10-28:
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean.
- `cargo test --workspace` passes 51 tests: 22 in domain, 15 in app, 10 in win (5 `keys` mappings, 5 overlay helpers), 4 in store.
- An injected hold-flick-release (`smoke.ps1`) ran the whole path on a real screen: `registered the hotkey Ctrl+Shift+Space`, `showing 4 wedges at (167, 467), scale 1`, `highlighting Some(1)`, `hiding the menu`, `stub: would paste 43 characters`. What still needs human eyes: the ring looks right, tap-then-click, Esc, a hotkey another app holds.

Toolchain at the time:
- Rust 1.99, edition 2024, resolver 3.
- `windows` **0.62**: failure-reporting calls return `Result`; `GetMessageW`/`PeekMessageW`/`ShowWindow` return `BOOL` with `.as_bool()`/`.ok()`; `GetAsyncKeyState` takes `i32`; `VK_*` are `VIRTUAL_KEY(u16)`; for a thread timer (`hwnd = NULL`) `SetTimer` generates its own id and `KillTimer` needs that returned id.
- `tiny-skia` **0.11.4**: `fill_path`/`stroke_path` take `(path, paint, [stroke,] Transform, Option<&Mask>)`; cubic curves are `PathBuilder::cubic_to`; data is premultiplied RGBA.
- `ab_glyph` **0.2.32**: `Font::outline(GlyphId) -> Option<Outline>` gives unscaled, unpositioned `OutlineCurve`s; metrics come via `as_scaled(PxScale)` + the `ScaleFont` trait.
- eframe/egui **0.37**: `eframe::App::ui(&mut self, ui: &mut egui::Ui, frame)` replaces `update`, `egui::Panel::bottom(id).show(ui, ..)` replaces `TopBottomPanel`, `CentralPanel::show` takes a `&mut Ui`.
- toml 1.x, thiserror 2, directories 6, tempfile 3.

What works:
- Domain model, validation, wedge maths, `Rect` + `place` (edge clamping), `Vec2 / f64` scaling.
- The `Core` state machine, dividing cursor offsets by the `Placement`'s scale before `pick`/`contains`.
- The TOML store.
- The settings window: edit, reorder, add and delete snippets; hotkey text with live validation and a warning for Alt; dead zone, radius and modifier wait; Save and Revert.
- Saving from the settings window sends `Event::ConfigChanged` to the input thread.
- The hotkey: registered on the input thread, a new binding on id 1/2 before the old one is dropped, `AlreadyRegistered` typed for 1409; Esc on id 3 only while the menu is up.
- The input thread: a `GetMessageW` loop woken by `PostThreadMessageW`; a 10 ms `SetTimer` while `wants_ticks` samples cursor, hotkey key, modifiers and the primary button (`SM_SWAPBUTTON`) into `Tick` and `Click`; `InputHandle::quit` posts `WM_QUIT`.
- The overlay: a `WS_EX_LAYERED | WS_EX_NOACTIVATE` window on the input thread, never taking focus (`WM_MOUSEACTIVATE` → `MA_NOACTIVATE`, `SW_SHOWNOACTIVATE`); monitor work area via `GetMonitorInfoW`, scale via `GetDpiForMonitor`, drawing with tiny-skia (disc, wedges, separators, dead-zone and rim circles, labels in Segoe UI via ab_glyph, cut to the wedge's chord), upload with `UpdateLayeredWindow` over a `CreateDIBSection` BGRA buffer; the process is Per-Monitor V2 DPI aware from the first line of `main`.

What is a stub (it logs and does nothing):
- `radial_win::inject::ClipboardPaste`

Known gaps:
- No tray icon. Closing the settings window quits (via `InputHandle::quit`).
- `autostart` is stored but not applied.
- Hotkey registration errors are only logged.
- egui uses its default wgpu renderer, which is heavy and builds slowly.
- The overlay has only been seen through logs and an automated flick; nobody has judged the drawing yet (ring geometry, label fit, highlight colour, multi-DPI).

## 4. Code map

```
Cargo.toml            workspace; lints: clippy unwrap_used/expect_used = warn (allowed in tests via clippy.toml)
crates/domain/      radial-domain: pure, forbid(unsafe)
  config.rs           Config{version, hotkey, modifier_release_timeout_ms, autostart, layout, snippets(#[serde(rename="snippet")])}
                      Snippet{label,text}; validate() -> ConfigError; CONFIG_VERSION, MAX_SNIPPETS, MAX_LABEL_CHARS
  hotkey.rs           Modifiers{ctrl,shift,alt}; Key{Letter,Digit,F,Space,Backquote}; HotkeyBinding (valid by construction,
                      FromStr/Display "Ctrl+Shift+Space", serde as string); HotkeyError
  layout.rs           Vec2 (Sub, Div<f64>); Layout{dead_zone,radius} in logical 96-DPI pixels: pick(count, offset)
                      (wedge 0 at 12 o'clock, clockwise), contains(offset), label_offset(count, i), is_valid();
                      Rect + place(cursor, area, radius) moves the centre inward, never panics
crates/app/           radial-app: depends on domain only, forbid(unsafe)
  ports.rs            ConfigRepository, MenuPresenter{show(cursor,..)->Placement{centre, scale}, highlight, hide},
                      TextInjector, HotkeyRegistrar{register, defaulted register_escape/unregister_escape}
  machine.rs          Event{HotkeyPressed, Tick{now,cursor,key_down,modifiers_down}, Click{now,cursor}, Cancel, SetPaused, ConfigChanged}
                      State{Idle, Holding{origin,scale}, Open{origin,scale}, Releasing{text,deadline}};
                      Core::{new,start,handle,wants_ticks,config,is_paused,hotkeys_mut}; offsets are (cursor-origin)/scale;
                      CoreError{Paste, Hotkey}; tests with fakes recording calls
  settings.rs         save_config(repo, &config) -> Result<(), SaveError<E>> (validates, then saves)
crates/store/         radial-store: TomlConfigRepository::{at_default_location, at, path}; StoreError
crates/win/           radial-win: `windows` 0.62 + tiny-skia + ab_glyph + thiserror; unsafe in small wrappers with SAFETY notes
  keys.rs             pure `Key`→VK, `Modifiers`→`MOD_* | MOD_NOREPEAT`, `Modifiers`→poll VKs; unit-tested
  hotkey.rs           `Hotkeys`: `RegisterHotKey(None, ..)` on the input thread; the snippet hotkey
                      alternates ids 1/2, new before old is dropped; Esc is id 3; 1409 becomes
                      `HotkeyError::AlreadyRegistered`
  input.rs            `input::spawn(core) -> InputHandle{tx, thread_id}`; `GetMessageW` loop:
                      `WM_HOTKEY` (id 1/2 → HotkeyPressed, 3 → Cancel), `WM_APP+1` drains the channel,
                      `WM_TIMER` (SetTimer 10 ms while `wants_ticks`, keeping the returned id for KillTimer, with Esc)
                      samples cursor, key, modifiers, primary button into `Click` then `Tick`; `quit()` posts `WM_QUIT`
  overlay.rs          `Overlay`: window on the input thread (WS_EX_LAYERED|NOACTIVATE|TOPMOST|TOOLWINDOW, WS_POPUP,
                      WM_MOUSEACTIVATE → MA_NOACTIVATE); show: `MonitorFromPoint` + `GetMonitorInfoW` work area,
                      `GetDpiForMonitor` scale, `place` centre, tiny-skia disc/wedges/separators/circles/labels,
                      `UpdateLayeredWindow` over a CreateDIBSection BGRA buffer (DcGuard frees GDI handles),
                      `SW_SHOWNOACTIVATE` + topmost; highlight redraws, hide hides; labels: segoeui.ttf via ab_glyph,
                      `cut_to_fit` to the wedge chord; helpers tested: `wedge_angles`, `rgba_to_bgra`, `cut_to_fit`
  dpi.rs              `enable_per_monitor_v2()`, called first in `main`
  inject.rs           stub (M3)
  ../../smoke.ps1     injects a hold-flick-release against the built exe and prints the logs; dev helper for the
                      manual tests, not part of the app
crates/settings/      radial-settings: run(config, save: FnMut(&Config)->Result<(),String>) -> eframe::Result
crates/desktop/       radial-paste (bin): composition root; enables Per-Monitor V2 DPI first, env_logger filter "warn,radial=info,wgpu_hal=off";
                      input::spawn(Core + real hotkeys + overlay) on the "input" thread, settings on the main thread,
                      which sends ConfigChanged through the InputHandle and quits it when the window closes
```

### How `Core` behaves (already tested)

- **`HotkeyPressed`:**
  - When Idle, not paused and with at least one snippet: `menu.show()`, then **Holding**. The origin is the centre that `show` returns, which may be moved inward near a screen edge.
  - When **Open**, pressing the hotkey closes the menu.
- **`Tick` while Holding:**
  - Key still down: update the highlight (only when it changes).
  - Key released over a wedge: `hide`, then **Releasing**. A flick far past the radius still picks.
  - Key released in the dead zone: **Open**.
- **`Tick` while Open:** highlight the wedge under the cursor while it is inside the radius.
- **`Click` while Open:** a click on a wedge inside the radius picks it. A click in the centre or outside the ring closes the menu.
- **`Tick` while Releasing:** paste once the modifiers are up or the deadline has passed, then Idle.
- **`Cancel`:** closes from any state. While Releasing it just drops the paste, since the menu is already hidden.
- **`ConfigChanged`:** closes the menu. If the hotkey changed, register it; if that fails, keep the old hotkey but accept the rest of the config, and return `CoreError::Hotkey`.

## 5. Open decisions (settle these before building the relevant part)

1. **Settings in-process or as a second process.** This matters at M4.
   - Problem: `eframe::run_native` can only run once per process, because a winit `EventLoop` can't be recreated. So a resident tray app can't close and reopen the window cheaply. Keeping eframe alive with a hidden window costs about 100 MB with wgpu, and a hidden window may stop calling `ui`.
   - **Recommended:** two modes of one exe.
     - `radial-paste.exe` stays resident: tray, input thread and overlay, no egui.
     - `radial-paste.exe --settings` is the eframe window. It saves the TOML, then posts a registered window message (`RegisterWindowMessageW("radial-paste.reload")`) to the resident's hidden message window. The resident reloads the file and feeds `ConfigChanged`.
   - The resident reports hotkey failures with a tray balloon.
2. **Units of `Layout`.** Settled for M2, as recommended: `Layout` stays in logical (96-DPI) pixels. `MenuPresenter::show` returns `Placement { centre: Vec2, scale: f64 }` (scale = monitor DPI / 96), and `Core` divides offsets by `scale` before `pick` and `contains`. Done in `ports.rs`, `machine.rs` and the tests.
3. **egui renderer.** Recommended: `eframe = { default-features = false, features = ["glow", "default_fonts"] }` (check the 0.37 feature names). It is lighter and builds faster.
4. **Font for the overlay labels.** Settled for M2, as recommended: `ab_glyph`, loading `C:\Windows\Fonts\segoeui.ttf` at the first `show`. If it is missing, it logs once and draws the ring without labels.
5. **How much clipboard to restore.** Recommended for v1: every `HGLOBAL`-backed format, skipping GDI-handle formats (`CF_BITMAP`, `CF_ENHMETAFILE`, `CF_METAFILEPICT`, `CF_PALETTE`, owner-display). `CF_DIB` still covers images.

## 6. Milestones

### M1: real hotkey and input loop (`crates/win`) ← done 2026-10-28

1. Add the Windows bindings with `cargo add -p radial-win windows --features Win32_Foundation,Win32_UI_WindowsAndMessaging,Win32_UI_Input_KeyboardAndMouse,Win32_System_Threading`. Add more features as later steps need them.
2. Add `keys.rs`, a pure mapping with unit tests:
   - `Key` → virtual key: letters → `'A'..'Z'`, digits → `'0'..'9'`, `F(n)` → `VK_F1 + n - 1`, Space → `VK_SPACE`, Backquote → `VK_OEM_3`.
   - `Modifiers` → `MOD_CONTROL | MOD_SHIFT | MOD_ALT`, always with `MOD_NOREPEAT`.
   - Modifiers → the VKs to poll: `VK_CONTROL`, `VK_SHIFT`, `VK_MENU`.
3. Make `Hotkeys` real:
   - `RegisterHotKey(None, id, ..)`. With no window it is tied to the calling thread, so register on the input thread only.
   - Switch ids between 1 and 2: register the new binding first, then `UnregisterHotKey` the old one.
   - Return a typed error: `ERROR_HOTKEY_ALREADY_REGISTERED` (1409) becomes "used by another app".
   - Add `register_escape()` and `unregister_escape()` with id 3, `VK_ESCAPE`, no modifiers.
4. Rewrite `input.rs` around a Win32 message loop:
   - At thread start, call `PeekMessageW(PM_NOREMOVE)` so the thread has a message queue. Then send `GetCurrentThreadId()` back to the spawner, for example over a one-shot `mpsc`.
   - Replace the raw `mpsc::Sender<Event>` handed to other threads with `InputHandle { tx, thread_id }`. Its `send` does `tx.send(event)` and then `PostThreadMessageW(thread_id, WM_APP + 1, ..)`. On `WM_APP + 1`, drain `try_recv` into `core.handle`.
   - On `WM_HOTKEY`:
     - id 1 or 2: `GetCursorPos` → `Event::HotkeyPressed`.
     - id 3: `Event::Cancel`.
   - After every handled message:
     - If `core.wants_ticks()` and there is no timer, call `SetTimer(None, 0, 10, None)` and register Esc.
     - If there are no wanted ticks and a timer exists, call `KillTimer` and unregister Esc.
   - On `WM_TIMER`, sample state into `Event::Tick`:
     - `now = Instant::now()` and `GetCursorPos`.
     - `key_down = GetAsyncKeyState(vk) < 0` (high bit set).
     - `modifiers_down` is true if any modifier in the binding is down.
   - Clicks: on the same timer, detect the transition of the primary mouse button from up to down. `GetAsyncKeyState` reads *physical* buttons, so pick `VK_LBUTTON` or `VK_RBUTTON` with `GetSystemMetrics(SM_SWAPBUTTON)`. Send `Event::Click` before the `Tick`.
   - Always `TranslateMessage` and `DispatchMessageW`, so the overlay's window procedure runs (M2).
   - Leave the loop on `WM_QUIT`. Add a quit path: `InputHandle::quit()` posts `WM_QUIT`.
5. Update `desktop/main.rs` to use `InputHandle`.
6. Manual test: run it, hold the hotkey and flick. The logs should show show, highlight, hide and paste in order. Also test tap-then-click, Esc, and changing the hotkey in settings to one that another app holds.

All `unsafe` gets `// SAFETY:` comments. Keep the `windows` calls in small wrapper functions so the loop logic stays readable.

### M2: overlay (`crates/win/overlay.rs`) ← done 2026-10-28

1. ~~Settle open decision 2~~ Done: `Placement { centre, scale }`, adopted as recommended.
2. ~~Pure placement maths~~ Done: `domain::place` + `Rect`, with tests.
3. ~~Window~~ Done.
4. ~~`show`~~ Done.
5. ~~Drawing~~ Done (wedges as polylined arcs; labels via ab_glyph outlines → tiny-skia paths).
6. ~~`highlight`/`hide`~~ Done.
7. ~~DPI awareness~~ Done: `radial_win::dpi::enable_per_monitor_v2()` first in `main`; eframe asks for the same later, which is harmless.
8. ~~Pure helpers tested~~ Done: `wedge_angles`, `rgba_to_bgra`, `cut_to_fit`, plus `place` in the domain.

Remaining from M2: a human should look at the ring once (geometry, labels, highlight, a HiDPI monitor), and try tap-then-click and Esc against the real overlay.

### M3: paste (`crates/win/inject.rs`) ← next

1. Add the features `Win32_System_DataExchange`, `Win32_System_Memory` and `Win32_System_Ole` (for `CF_UNICODETEXT`).
2. `OpenClipboard(None)`, retrying up to 10 times about 10 ms apart, because other apps hold it briefly. Always close it, using an RAII guard.
3. Save the clipboard: `EnumClipboardFormats`, and copy each `HGLOBAL` format's bytes (open decision 5).
4. Set the text:
   - `EmptyClipboard`.
   - `SetClipboardData(CF_UNICODETEXT, ..)`: UTF-16 with a NUL terminator, `\n` turned into `\r\n`, in `GlobalAlloc(GMEM_MOVEABLE)`.
   - Also set the registered formats `ExcludeClipboardContentFromMonitorProcessing` (empty), `CanIncludeInClipboardHistory` (DWORD 0) and `CanUploadToCloudClipboard` (DWORD 0).
   - Remember `GetClipboardSequenceNumber()`.
5. `SendInput`: Ctrl down, V down, V up, Ctrl up. If fewer than 4 events were inserted, return an error. UIPI blocking is *not* reported, so it fails silently. Later, detect an elevated foreground window and warn.
6. Wait about 100 ms, then restore the saved formats, but only if the sequence number hasn't changed (so you don't wipe something the user copied in between). For v1 a blocking sleep on the input thread is acceptable, since the core is Idle; a timer-driven restore can come later.
7. Pure helpers to unit-test: CRLF conversion and UTF-16 encoding.
8. Manual matrix:
   - Notepad
   - VS Code
   - Chrome text field
   - Word
   - Windows Terminal (expect failure: paste there is Ctrl+Shift+V)
   - Elevated cmd (expect a silent failure from UIPI)
   - Check that Win+V history doesn't show the snippet.
   - Check that an image on the clipboard survives.

### M4: tray, lifetime, single instance, autostart

1. Settle open decision 1. The rest of this assumes the recommended two-process split.
2. Resident mode (no arguments):
   - Single instance: create the named mutex `CreateMutexW("Local\\radial-paste")`. If it already exists, post the reload/open message to the running copy and exit.
   - A hidden message-only window (`HWND_MESSAGE`) on the input thread receives `radial-paste.reload` and `radial-paste.open-settings`.
   - Tray (`tray-icon` + `muda`) on the input thread, which already has a message loop. Menu items: Settings…, Pause (checkbox → `Event::SetPaused`), Quit (`WM_QUIT`). Double-clicking the icon opens settings.
   - Settings… starts `current_exe() --settings`, unless a settings window is already open: then bring it to the front, e.g. by checking with a second named mutex.
   - On reload: `repo.load()`, then `ConfigChanged`. Show any `CoreError` as a tray balloon.
3. Settings mode (`--settings`): today's `radial_settings::run`. Its `save` closure does `save_config`, then `PostMessageW(FindWindowExW(HWND_MESSAGE, .., class), reload_msg)`. It does not need the input thread.
4. Autostart: when the config loads or reloads, sync `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value `radial-paste` = `"<exe path>"`, or delete it. Use the `windows` registry API or the `winreg` crate.
5. Remove the "not wired up yet" note from the autostart checkbox.

### M5: polish and shipping

- Embed an app manifest and icon (`embed-resource` or `winresource`): Per-Monitor V2 DPI, `asInvoker`, Common Controls v6.
- Log to a file under `%LOCALAPPDATA%\radial-paste\logs` in release builds, because `windows_subsystem = "windows"` hides the console.
- If `config.toml` fails to load, keep a copy as `config.toml.bad` before the defaults overwrite it.
- Switch eframe to glow (open decision 3).
- Installer with `cargo-wix` (MSI). Code-sign the exe and the MSI, because of SmartScreen and antivirus heuristics on `SendInput`.
- CI on GitHub Actions `windows-latest`: fmt, clippy `-D warnings`, test, and a release build uploaded as an artifact.

### Later (not v1)

- More than 8 snippets: two rings (distance picks the ring) or pages (mouse wheel). Both are only layout functions in `domain::layout`, plus validation limits.
- Typing the text with `SendInput` + `KEYEVENTF_UNICODE` as a per-snippet paste method, which leaves the clipboard alone.
- Placeholders: `{date}`, `{time}`, `{clipboard}`, and a cursor position after the paste.
- A mouse-button trigger. This needs `WH_MOUSE_LL`; ask first, per `AGENTS.md`.
- Import and export of snippets.

## 7. Risks to keep in mind

- **UIPI:** pasting into elevated windows fails silently. Document it, or warn when the foreground window is elevated.
- **Terminals** paste with Ctrl+Shift+V. A later fix is a per-app paste chord, or typing the text.
- **An Alt in the hotkey** can open the target app's menu bar when released. The settings window already warns; a "menu mask" key (VK `0xE8`) is a possible fix.
- **The clipboard restore races** the target app reading the clipboard. Tune the delay; the sequence-number check prevents clobbering.
- **Security software** may flag `SendInput`. Signing the exe helps.
- **eframe can't be restarted in one process**, hence open decision 1.
- **DPI:** virtualised coordinates if the process isn't Per-Monitor V2 aware.

## 8. Next step

Start **M3** (`crates/win/inject.rs`): clipboard save, `CF_UNICODETEXT` with the history-exclusion formats, `SendInput` Ctrl+V, sequence-number-checked restore. Add the `Win32_System_DataExchange`, `Win32_System_Memory` and `Win32_System_Ole` features. Before that, a human should eyeball the overlay once: run the app, hold the hotkey, and check the ring, the labels, the highlight, tap-then-click and Esc.

Don't commit until the user asks. Nothing is committed yet, so offer an initial commit (`chore: scaffold workspace`) first.

Checks before finishing any change: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
