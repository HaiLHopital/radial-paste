//! Reading the physical mouse buttons and the modifier keys with `GetAsyncKeyState`.
//!
//! The presses that open the menu must be seen even when another window has focus, and those
//! presses never reach our message queue: so the buttons are sampled, not hooked. The same
//! sampler serves the input thread's watch mode for mouse-button hotkeys and the settings
//! window's press-to-record capture.

use radial_domain::{Key, Modifiers, MouseButton};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LBUTTON, VK_MBUTTON, VK_MENU, VK_RBUTTON, VK_SHIFT,
    VK_XBUTTON1, VK_XBUTTON2,
};

use crate::keys;

/// The mouse button held down right now, if any. The buttons that may stand alone in a hotkey
/// are asked first, so that a stray chord - a finger resting on the left button, say - records
/// the likelier intent instead of a button the domain would reject.
pub fn pressed_button() -> Option<MouseButton> {
    [
        (MouseButton::Middle, u32::from(VK_MBUTTON.0)),
        (MouseButton::Back, u32::from(VK_XBUTTON1.0)),
        (MouseButton::Forward, u32::from(VK_XBUTTON2.0)),
        (MouseButton::Left, u32::from(VK_LBUTTON.0)),
        (MouseButton::Right, u32::from(VK_RBUTTON.0)),
    ]
    .into_iter()
    .find(|(_, vk)| is_down(*vk))
    .map(|(button, _)| button)
}

/// The modifier keys held down right now.
pub fn modifiers_down() -> Modifiers {
    Modifiers {
        ctrl: is_down(u32::from(VK_CONTROL.0)),
        shift: is_down(u32::from(VK_SHIFT.0)),
        alt: is_down(u32::from(VK_MENU.0)),
    }
}

/// Whether `button` is physically down.
pub(crate) fn is_button_down(button: MouseButton) -> bool {
    is_down(keys::virtual_key(Key::Mouse(button)))
}

/// Whether the key is physically down: `GetAsyncKeyState`'s high bit.
pub(crate) fn is_down(vk: u32) -> bool {
    // SAFETY: any virtual-key code is accepted, and the call only reads global key state. The
    // parentheses keep the block from being parsed as a statement.
    (unsafe { GetAsyncKeyState(vk as i32) }) < 0
}
