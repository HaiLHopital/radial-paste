//! The input thread: owns the hotkey and the overlay, and runs the [`Core`] on a Win32 message
//! loop.
//!
//! The thread has no window. Its loop receives
//! - `WM_HOTKEY` from the registrations in [`crate::hotkey`]: the snippet hotkey becomes
//!   [`Event::HotkeyPressed`] with the cursor position, Esc becomes [`Event::Cancel`];
//! - `WM_TIMER` from a 10 ms thread timer. It runs while [`Core::wants_ticks`], sampling
//!   `GetCursorPos` and `GetAsyncKeyState` (the hotkey's key, its modifiers, and the primary
//!   mouse button) into [`Event::Click`] and [`Event::Tick`]. It also runs whenever the hotkey
//!   is a mouse button, which [`crate::hotkey`] cannot register: each tick watches for the
//!   whole combo's press, which opens the menu while idle and closes it again on a second
//!   press, like the keyboard hotkey's `WM_HOTKEY` does.
//! - a wake-up message when another thread sends an [`Event`] over an [`InputHandle`];
//! - `WM_QUIT` from [`InputHandle::quit`], which ends the loop.
//!
//! A mouse-button hotkey is watched for by polling, like everything else here: no low-level
//! mouse hook. The press is therefore not swallowed, and also reaches the app under the cursor;
//! the middle, back and forward buttons, which carry no ordinary click, are the best choices.

use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Instant;

use radial_app::ports::{HotkeyRegistrar, MenuPresenter, TextInjector};
use radial_app::{Core, Event};
use radial_domain::{Key, Vec2};
use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, KillTimer, MSG, PM_NOREMOVE,
    PeekMessageW, PostThreadMessageW, SM_SWAPBUTTON, SetTimer, TranslateMessage, WM_APP, WM_HOTKEY,
    WM_QUIT, WM_TIMER,
};

use crate::hotkey::{ESCAPE_ID, HOTKEY_ID, OTHER_HOTKEY_ID};
use crate::{keys, mouse};

/// The id of the 10 ms timer that samples the cursor and the keys.
const TICK_TIMER: usize = 1;
/// The milliseconds between two ticks.
const TICK_MS: u32 = 10;
/// Wakes the input thread so that it drains the event channel.
const WAKE_UP: u32 = WM_APP + 1;

/// A way to talk to the input thread. Clone it to hand out more copies.
#[derive(Clone)]
pub struct InputHandle {
    tx: mpsc::Sender<Event>,
    thread_id: u32,
}

impl InputHandle {
    /// Queues `event` for the input thread and wakes it.
    pub fn send(&self, event: Event) {
        if self.tx.send(event).is_err() {
            log::warn!("the input thread is gone; dropping an event");
            return;
        }
        // SAFETY: `spawn` publishes the thread id only after `PeekMessageW` has given the thread
        // a message queue, and the thread only ends on `WM_QUIT`, so the id stays postable.
        if let Err(e) = unsafe { PostThreadMessageW(self.thread_id, WAKE_UP, WPARAM(0), LPARAM(0)) }
        {
            log::warn!("could not wake the input thread: {e}");
        }
    }

    /// Ends the input thread's loop with `WM_QUIT`.
    pub fn quit(&self) {
        // SAFETY: as in `send`: the id belongs to the input thread, which has a message queue.
        if let Err(e) = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
        {
            log::warn!("could not tell the input thread to quit: {e}");
        }
    }
}

/// An error from starting the input thread.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("could not spawn the input thread: {0}")]
    Thread(#[from] std::io::Error),
    #[error("the input thread did not report its message queue")]
    Ready,
}

/// Spawns the input thread, which owns `core` and runs until [`InputHandle::quit`] is called.
pub fn spawn<M, I, H>(mut core: Core<M, I, H>) -> Result<InputHandle, SpawnError>
where
    M: MenuPresenter + Send + 'static,
    I: TextInjector + Send + 'static,
    H: HotkeyRegistrar + Send + 'static,
{
    let (tx, inbox) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();

    thread::Builder::new()
        .name("input".into())
        .spawn(move || {
            // Give the thread a message queue before publishing its id, so that the first
            // `PostThreadMessageW` cannot race with the queue's creation.
            // SAFETY: peeking with `PM_NOREMOVE` only probes the queue; no window is needed.
            let _ = unsafe { PeekMessageW(&mut MSG::default(), None, 0, 0, PM_NOREMOVE) };
            // SAFETY: returns the id of this thread, which stays valid for `PostThreadMessageW`
            // as long as the thread lives.
            let thread_id = unsafe { GetCurrentThreadId() };
            if ready_tx.send(thread_id).is_err() {
                return; // The spawner is gone; with it, nobody could ever shut us down.
            }
            run(&mut core, inbox);
        })
        .map_err(SpawnError::Thread)?;

    let thread_id = ready_rx.recv().map_err(|_| SpawnError::Ready)?;
    Ok(InputHandle { tx, thread_id })
}

fn run<M, I, H>(core: &mut Core<M, I, H>, inbox: Receiver<Event>)
where
    M: MenuPresenter,
    I: TextInjector,
    H: HotkeyRegistrar,
{
    if let Err(e) = core.start() {
        log::error!("{e}");
    }

    let mut timer: Option<usize> = None;
    let mut esc_up = false;
    let mut button_was_down = false;
    let mut trigger_was_down = false;
    // A watched mouse button needs the timer while nothing is happening, and no message would
    // arrive to start it: so the first sync happens before the loop blocks on `GetMessageW`.
    sync(core, &mut timer, &mut esc_up);
    loop {
        let mut msg = MSG::default();
        // SAFETY: `msg` is an out parameter; the `None` window and the 0..0 range select every
        // message of this thread's queue.
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if got.0 == 0 {
            break; // WM_QUIT
        }
        if got.0 == -1 {
            log::error!("GetMessageW failed; ending the input loop");
            break;
        }

        match msg.message {
            WM_HOTKEY => on_hotkey(core, msg.wParam.0),
            WAKE_UP => drain(core, &inbox),
            _ => {}
        }
        // A stale `WM_TIMER` can still sit in the queue after the timer was stopped; without a
        // timer there is nothing to sample.
        if msg.message == WM_TIMER && timer.is_some() {
            // The combo's edge is watched on every tick, so that a second press of a
            // mouse-button hotkey closes the menu, like the keyboard hotkey's `WM_HOTKEY`.
            // The core ignores the press while Holding or Releasing, where the button is
            // already down anyway.
            trigger_was_down = watch(core, trigger_was_down, &mut button_was_down);
            if core.wants_ticks() {
                button_was_down = tick(core, button_was_down);
            }
        }

        sync(core, &mut timer, &mut esc_up);

        // SAFETY: `msg` was filled by `GetMessageW` above. Dispatching also runs the window
        // procedures of later adapters, such as the overlay's.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Keeps the timer and Esc in step with the core. The 10 ms timer runs while the menu wants
/// ticks, or while a mouse-button hotkey is watched for; Esc is registered only while the menu
/// is up, so that it cannot get in the way otherwise.
fn sync<M, I, H>(core: &mut Core<M, I, H>, timer: &mut Option<usize>, esc_up: &mut bool)
where
    M: MenuPresenter,
    I: TextInjector,
    H: HotkeyRegistrar,
{
    let menu_up = core.wants_ticks();
    if menu_up != *esc_up {
        if menu_up {
            if let Err(e) = core.hotkeys_mut().register_escape() {
                log::error!("could not register Esc: {e}");
            }
        } else {
            core.hotkeys_mut().unregister_escape();
        }
        *esc_up = menu_up;
    }

    let watching = watches_mouse(core) && !core.is_paused();
    if menu_up || watching {
        if timer.is_none() {
            *timer = start_ticks();
        }
    } else if let Some(id) = timer.take() {
        stop_ticks(id);
    }
}

/// Whether the hotkey's key is a mouse button, which `RegisterHotKey` cannot take.
fn watches_mouse<M, I, H>(core: &Core<M, I, H>) -> bool
where
    M: MenuPresenter,
    I: TextInjector,
    H: HotkeyRegistrar,
{
    matches!(core.config().hotkey.key(), Key::Mouse(_))
}

/// Turns a `WM_HOTKEY` into an event: the snippet hotkey (on either of its two ids) opens or
/// closes the menu, Esc cancels it.
fn on_hotkey<M, I, H>(core: &mut Core<M, I, H>, id: usize)
where
    M: MenuPresenter,
    I: TextInjector,
    H: HotkeyRegistrar,
{
    match id as i32 {
        HOTKEY_ID | OTHER_HOTKEY_ID => handle(
            core,
            Event::HotkeyPressed {
                cursor: cursor_pos(),
            },
        ),
        ESCAPE_ID => handle(core, Event::Cancel),
        _ => {}
    }
}

/// Handles every queued channel event.
fn drain<M, I, H>(core: &mut Core<M, I, H>, inbox: &Receiver<Event>)
where
    M: MenuPresenter,
    I: TextInjector,
    H: HotkeyRegistrar,
{
    while let Ok(event) = inbox.try_recv() {
        handle(core, event);
    }
}

/// Starts the 10 ms timer. Returns the timer's id, which with no window is generated by
/// `SetTimer` and needed again by `KillTimer`; `None` on failure.
fn start_ticks() -> Option<usize> {
    // SAFETY: with no window this is a thread timer, and its `WM_TIMER` arrives in the loop that
    // called us.
    let id = unsafe { SetTimer(None, TICK_TIMER, TICK_MS, None) };
    if id == 0 {
        log::error!("could not start the tick timer");
        return None;
    }
    Some(id)
}

fn stop_ticks(id: usize) {
    // SAFETY: the timer was created by `start_ticks` on this same thread, with no window, and
    // `id` is what `SetTimer` returned for it.
    if let Err(e) = unsafe { KillTimer(None, id) } {
        log::warn!("could not stop the tick timer: {e}");
    }
}

/// Samples the cursor and the keys into a `Tick`, and the primary mouse button's down-edge into
/// a `Click` sent just before it, because a click should be judged on the menu state before the
/// cursor moves on. Returns whether the button was down, for the next edge detection.
fn tick<M, I, H>(core: &mut Core<M, I, H>, button_was_down: bool) -> bool
where
    M: MenuPresenter,
    I: TextInjector,
    H: HotkeyRegistrar,
{
    let binding = core.config().hotkey;
    let now = Instant::now();
    let cursor = cursor_pos();

    let button_down = mouse::is_down(primary_button_vk());
    if button_down && !button_was_down {
        handle(core, Event::Click { now, cursor });
    }

    handle(
        core,
        Event::Tick {
            now,
            cursor,
            key_down: mouse::is_down(keys::virtual_key(binding.key())),
            modifiers_down: keys::modifier_vks(binding.modifiers()).any(mouse::is_down),
        },
    );

    button_down
}

/// Watches for the press of the whole combo of a mouse-button hotkey and opens the menu on its
/// down-edge, so that holding the button through the paste does not reopen the menu at once;
/// while the menu is up, a fresh press closes it again. Returns whether the combo was down, for
/// the next edge detection.
fn watch<M, I, H>(core: &mut Core<M, I, H>, was_down: bool, button_was_down: &mut bool) -> bool
where
    M: MenuPresenter,
    I: TextInjector,
    H: HotkeyRegistrar,
{
    let binding = core.config().hotkey;
    let Key::Mouse(button) = binding.key() else {
        return false;
    };
    let down = mouse::is_button_down(button)
        && keys::modifier_vks(binding.modifiers()).all(mouse::is_down);
    if down && !was_down {
        // The trigger button is still held as the menu comes up; do not let its first menu-phase
        // sample count as a click.
        *button_was_down = true;
        handle(
            core,
            Event::HotkeyPressed {
                cursor: cursor_pos(),
            },
        );
    }
    down
}

fn handle<M, I, H>(core: &mut Core<M, I, H>, event: Event)
where
    M: MenuPresenter,
    I: TextInjector,
    H: HotkeyRegistrar,
{
    if let Err(e) = core.handle(event) {
        log::error!("{e}");
    }
}

/// The cursor in physical screen pixels. On the rare failure, the origin.
fn cursor_pos() -> Vec2 {
    let mut p = POINT::default();
    // SAFETY: `p` is a valid out parameter, left untouched on failure.
    if unsafe { GetCursorPos(&mut p) }.is_ok() {
        Vec2::new(f64::from(p.x), f64::from(p.y))
    } else {
        Vec2::new(0.0, 0.0)
    }
}

/// The virtual key of the physical primary button, so that a click survives swapped mouse
/// buttons: `GetAsyncKeyState` reads physical buttons, not logical ones.
fn primary_button_vk() -> u32 {
    // SAFETY: `GetSystemMetrics` only reads a system value.
    if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 {
        u32::from(VK_RBUTTON.0)
    } else {
        u32::from(VK_LBUTTON.0)
    }
}
