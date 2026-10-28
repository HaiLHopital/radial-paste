//! The global hotkeys.
//!
//! `RegisterHotKey(NULL, id, ..)` ties a hotkey to the calling thread, so the hotkeys are
//! registered on the input thread only: that thread's message loop receives the `WM_HOTKEY`.
//! The snippet hotkey switches between two ids, and a new binding is registered before the old
//! one is let go, so that a failure leaves the old hotkey working.

use windows::Win32::Foundation::ERROR_HOTKEY_ALREADY_REGISTERED;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_ESCAPE,
};
use windows::core::HRESULT;

use radial_app::ports::HotkeyRegistrar;
use radial_domain::HotkeyBinding;

use crate::keys;

/// The id that currently holds the snippet hotkey; [`crate::input`] matches `WM_HOTKEY` on it.
pub(crate) const HOTKEY_ID: i32 = 1;
/// The other id, which the next registration uses.
pub(crate) const OTHER_HOTKEY_ID: i32 = 2;
/// Esc, registered only while the menu is up.
pub(crate) const ESCAPE_ID: i32 = 3;

#[derive(Debug, thiserror::Error)]
pub enum HotkeyError {
    #[error("the hotkey is used by another app")]
    AlreadyRegistered,
    #[error(transparent)]
    Other(#[from] windows::core::Error),
}

#[derive(Debug, Default)]
pub struct Hotkeys {
    /// Which of the two ids holds the snippet hotkey right now.
    slot: i32,
    current: Option<HotkeyBinding>,
}

impl Hotkeys {
    pub fn current(&self) -> Option<HotkeyBinding> {
        self.current
    }
}

impl HotkeyRegistrar for Hotkeys {
    type Error = HotkeyError;

    fn register(&mut self, binding: HotkeyBinding) -> Result<(), Self::Error> {
        let next = if self.slot == HOTKEY_ID {
            OTHER_HOTKEY_ID
        } else {
            HOTKEY_ID
        };
        // Register the new binding before letting go of the old one: if this fails, the old
        // hotkey keeps working.
        register_raw(
            next,
            keys::register_modifiers(binding.modifiers()),
            keys::virtual_key(binding.key()),
        )?;
        if self.current.is_some() {
            unregister_raw(self.slot);
        }
        self.slot = next;
        self.current = Some(binding);
        log::info!("registered the hotkey {binding}");
        Ok(())
    }

    fn register_escape(&mut self) -> Result<(), Self::Error> {
        register_raw(ESCAPE_ID, MOD_NOREPEAT, u32::from(VK_ESCAPE.0))
    }

    fn unregister_escape(&mut self) {
        unregister_raw(ESCAPE_ID);
    }
}

fn register_raw(id: i32, modifiers: HOT_KEY_MODIFIERS, vk: u32) -> Result<(), HotkeyError> {
    // SAFETY: with no window handle the hotkey belongs to the calling thread. Every call runs on
    // the input thread, the only thread that runs the message loop receiving `WM_HOTKEY`.
    match unsafe { RegisterHotKey(None, id, modifiers, vk) } {
        Ok(()) => Ok(()),
        Err(e) if e.code() == HRESULT::from_win32(ERROR_HOTKEY_ALREADY_REGISTERED.0) => {
            Err(HotkeyError::AlreadyRegistered)
        }
        Err(e) => Err(e.into()),
    }
}

fn unregister_raw(id: i32) {
    // SAFETY: the id was registered by `register_raw` on this same thread, with no window
    // handle, so the unregistering call must also pass none.
    if let Err(e) = unsafe { UnregisterHotKey(None, id) } {
        log::warn!("could not unregister the hotkey id {id}: {e}");
    }
}
