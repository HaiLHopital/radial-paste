//! The resident's plumbing: the single-instance mutex and a message-only window that carries
//! registered messages between processes.
//!
//! The settings window runs in its own process, because eframe's event loop cannot be
//! restarted inside one (open decision 1). It saves the config and posts `reload` here; the
//! resident re-reads the file and feeds [`radial_app::Event::ConfigChanged`] to the input
//! thread. A second launch of the exe finds the window and asks for the settings window
//! instead of starting a second resident.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowExW, GetMessageW, HWND_MESSAGE,
    MSG, PostMessageW, RegisterClassW, RegisterWindowMessageW, TranslateMessage, WNDCLASSW,
};
use windows::core::{Error as WinError, PCWSTR};

/// The window class of the resident's message-only window.
const HOST_CLASS: &str = "radial-paste.host";
/// The mutex that only the resident holds.
const INSTANCE_MUTEX: &str = "Local\\radial-paste";
/// The config file changed: the resident should re-read it.
const RELOAD_MESSAGE: &str = "radial-paste.reload";
/// The user wants the settings window.
const OPEN_SETTINGS_MESSAGE: &str = "radial-paste.open-settings";

/// What the host window was woken for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostMessage {
    Reload,
    OpenSettings,
}

/// What [`run_host`] should do after a pump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
}

/// Proves this process is the resident. Dropping it releases the mutex, which only matters if
/// another process were to start while this one is ending. The handle is never read; holding
/// it is the whole point.
#[derive(Debug)]
pub struct SingleInstance(#[allow(dead_code)] HANDLE);

#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    #[error("radial-paste is already running")]
    AlreadyRunning,
    #[error("could not create the single-instance mutex: {0}")]
    Create(WinError),
}

/// Creates the instance mutex. [`InstanceError::AlreadyRunning`] means another resident
/// exists; the caller should notify it ([`notify_open_settings`]) and exit.
pub fn try_acquire() -> Result<SingleInstance, InstanceError> {
    let name = wide(INSTANCE_MUTEX);
    // SAFETY: a named mutex with no security attributes and no initial owner. The handle is
    // deliberately never closed: it must live as long as the process is the resident.
    let handle = match unsafe { CreateMutexW(None, false, PCWSTR::from_raw(name.as_ptr())) } {
        Ok(handle) => handle,
        Err(source) => return Err(InstanceError::Create(source)),
    };
    // SAFETY: reads this thread's last error, which the call above set if the mutex already
    // existed; with no other error sources in between, it answers exactly our question.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Err(InstanceError::AlreadyRunning);
    }
    Ok(SingleInstance(handle))
}

/// Asks the running resident to open its settings window. Returns whether one answered.
pub fn notify_open_settings() -> bool {
    notify(OPEN_SETTINGS_MESSAGE)
}

/// Asks the running resident to re-read the config file. Returns whether it is running.
pub fn notify_reload() -> bool {
    notify(RELOAD_MESSAGE)
}

/// Posts `name`'s registered message to the resident's window, found by its class among the
/// message-only windows.
fn notify(message_name: &str) -> bool {
    let class = wide(HOST_CLASS);
    // SAFETY: only reads the class name; `HWND_MESSAGE` as the parent restricts the search to
    // message-only windows, which is where `run_host` creates the resident's window.
    let window = match unsafe {
        FindWindowExW(
            Some(HWND_MESSAGE),
            None,
            PCWSTR::from_raw(class.as_ptr()),
            None,
        )
    } {
        Ok(window) => window,
        Err(_) => return false,
    };
    let message = registered_message(message_name);
    // SAFETY: `window` came from the search above; posting to another process's window is how
    // registered messages cross process boundaries.
    unsafe { PostMessageW(Some(window), message, WPARAM(0), LPARAM(0)) }.is_ok()
}

/// Creates the message-only window and pumps its loop on the calling thread until the pump
/// asks for [`Flow::Quit`]. The tray icon lives on this thread too: its clicks arrive here as
/// ordinary window messages, keeping the loop awake while nothing else happens.
pub fn run_host(
    mut on_message: impl FnMut(HostMessage),
    mut on_pump: impl FnMut() -> Flow,
) -> Result<(), WinError> {
    register_class()?;
    create_window()?;
    let reload = registered_message(RELOAD_MESSAGE);
    let open = registered_message(OPEN_SETTINGS_MESSAGE);
    loop {
        let mut msg = MSG::default();
        // SAFETY: `msg` is an out parameter; the `None` window and the 0..0 range select every
        // message of this thread's queue.
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if got.0 == 0 || got.0 == -1 {
            break; // WM_QUIT or a failed message retrieval; nothing to salvage either way.
        }
        if msg.message == reload {
            on_message(HostMessage::Reload);
        } else if msg.message == open {
            on_message(HostMessage::OpenSettings);
        }
        // SAFETY: `msg` was filled by `GetMessageW` above. Dispatching also runs the tray
        // window's procedure, which feeds the tray's event channels.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if on_pump() == Flow::Quit {
            break;
        }
    }
    Ok(())
}

/// The system-wide id of a registered message: every process that asks for the same string
/// gets the same id, which is what lets the settings process talk to the resident.
fn registered_message(name: &str) -> u32 {
    let wide_name = wide(name);
    // SAFETY: only reads the name.
    unsafe { RegisterWindowMessageW(PCWSTR::from_raw(wide_name.as_ptr())) }
}

fn register_class() -> Result<(), WinError> {
    static REGISTERED: AtomicBool = AtomicBool::new(false);
    if REGISTERED.load(Ordering::Acquire) {
        return Ok(());
    }
    // SAFETY: `None` asks for this executable's module handle, which always exists.
    let module: HMODULE = unsafe { GetModuleHandleW(None) }?;
    let class_name = wide(HOST_CLASS);
    let class = WNDCLASSW {
        lpfnWndProc: Some(host_proc),
        hInstance: HINSTANCE(module.0),
        lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
        ..Default::default()
    };
    // SAFETY: `class` and the class name it points at live through the call, which only reads
    // them.
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(crate::last_error());
    }
    REGISTERED.store(true, Ordering::Release);
    Ok(())
}

/// The window procedure of the host window: it has no state, so the default procedure loses
/// nothing.
unsafe extern "system" fn host_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the parameters came from the message dispatch, which is the procedure's contract.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Creates the message-only window: parented to `HWND_MESSAGE`, it is invisible and never
/// shows up in any window lists, but it receives posted messages.
fn create_window() -> Result<HWND, WinError> {
    register_class()?;
    let class_name = wide(HOST_CLASS);
    let name = PCWSTR::from_raw(class_name.as_ptr());
    // SAFETY: the class is registered above; a message-only window needs no size, position or
    // style, and the process exits right after the host loop ends, so the window need not be
    // destroyed explicitly.
    let window = unsafe {
        CreateWindowExW(
            Default::default(),
            name,
            name,
            Default::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )?
    };
    Ok(window)
}

/// The string as a NUL-terminated UTF-16 buffer.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}
