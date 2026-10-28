//! The input thread: owns the hotkey and the overlay, and runs the [`Core`] on a Win32 message
//! loop.
//!
//! The thread has no window. Its loop receives
//! - `WM_HOTKEY` from the registrations in [`crate::hotkey`]: the snippet hotkey becomes
//!   [`Event::HotkeyPressed`] with the cursor position, Esc becomes [`Event::Cancel`];
//! - `WM_TIMER` from a 10 ms thread timer, run while [`Core::wants_ticks`]: each tick samples
//!   `GetCursorPos` and `GetAsyncKeyState` (the hotkey's key, its modifiers, and the primary
//!   mouse button) into [`Event::Click`] and [`Event::Tick`];
//! - a wake-up message when another thread sends an [`Event`] over an [`InputHandle`];
//! - `WM_QUIT` from [`InputHandle::quit`], which ends the loop.

use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Instant;

use radial_app::ports::{HotkeyRegistrar, MenuPresenter, TextInjector};
use radial_app::{Core, Event};
use radial_domain::Vec2;
use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, KillTimer, MSG, PM_NOREMOVE,
    PeekMessageW, PostThreadMessageW, SM_SWAPBUTTON, SetTimer, TranslateMessage, WM_APP, WM_HOTKEY,
    WM_QUIT, WM_TIMER,
};

use crate::hotkey::{ESCAPE_ID, HOTKEY_ID, OTHER_HOTKEY_ID};
use crate::keys;

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
    let mut button_was_down = false;
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
        if msg.message == WM_TIMER {
            button_was_down = tick(core, button_was_down);
        }

        if core.wants_ticks() {
            if timer.is_none() {
                timer = start_ticks(core);
            }
        } else if let Some(id) = timer.take() {
            stop_ticks(core, id);
        }

        // SAFETY: `msg` was filled by `GetMessageW` above. Dispatching also runs the window
        // procedures of later adapters, such as the overlay's.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
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

/// Starts the 10 ms timer and registers Esc. Returns the timer's id, which with no window is
/// generated by `SetTimer` and needed again by `KillTimer`; `None` on failure.
fn start_ticks<M, I, H>(core: &mut Core<M, I, H>) -> Option<usize>
where
    M: MenuPresenter,
    I: TextInjector,
    H: HotkeyRegistrar,
{
    // Esc is registered only while the menu is up, so that it cannot get in the way otherwise.
    if let Err(e) = core.hotkeys_mut().register_escape() {
        log::error!("could not register Esc: {e}");
    }
    // SAFETY: with no window this is a thread timer, and its `WM_TIMER` arrives in the loop that
    // called us.
    let id = unsafe { SetTimer(None, TICK_TIMER, TICK_MS, None) };
    if id == 0 {
        log::error!("could not start the tick timer");
        return None;
    }
    Some(id)
}

fn stop_ticks<M, I, H>(core: &mut Core<M, I, H>, id: usize)
where
    M: MenuPresenter,
    I: TextInjector,
    H: HotkeyRegistrar,
{
    // SAFETY: the timer was created by `start_ticks` on this same thread, with no window, and
    // `id` is what `SetTimer` returned for it.
    if let Err(e) = unsafe { KillTimer(None, id) } {
        log::warn!("could not stop the tick timer: {e}");
    }
    core.hotkeys_mut().unregister_escape();
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

    let button_down = is_down(primary_button_vk());
    if button_down && !button_was_down {
        handle(core, Event::Click { now, cursor });
    }

    handle(
        core,
        Event::Tick {
            now,
            cursor,
            key_down: is_down(keys::virtual_key(binding.key())),
            modifiers_down: keys::modifier_vks(binding.modifiers()).any(is_down),
        },
    );

    button_down
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

/// Whether the key is physically down: `GetAsyncKeyState`'s high bit.
fn is_down(vk: u32) -> bool {
    // SAFETY: any virtual-key code is accepted, and the call only reads global key state.
    let state = unsafe { GetAsyncKeyState(vk as i32) };
    state < 0
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
