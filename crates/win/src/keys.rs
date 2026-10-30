//! Pure mappings from the domain's hotkey types to the Win32 values the adapters need. No
//! `unsafe`, and every mapping is unit-tested against raw key codes, so a change in the
//! `windows` crate cannot silently break them.

use radial_domain::{Key, Modifiers, MouseButton};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, VK_CONTROL, VK_F1,
    VK_LBUTTON, VK_MBUTTON, VK_MENU, VK_OEM_3, VK_RBUTTON, VK_SHIFT, VK_SPACE, VK_XBUTTON1,
    VK_XBUTTON2,
};

/// The virtual-key code of `key`, for `RegisterHotKey` and `GetAsyncKeyState`. The mouse buttons
/// have virtual keys too, and `GetAsyncKeyState` answers for them, so a mouse hotkey needs no
/// other mapping.
pub fn virtual_key(key: Key) -> u32 {
    match key {
        Key::Letter(c) => u32::from(c),
        Key::Digit(d) => u32::from(b'0' + d),
        Key::F(n) => u32::from(VK_F1.0) + u32::from(n) - 1,
        Key::Space => u32::from(VK_SPACE.0),
        Key::Backquote => u32::from(VK_OEM_3.0),
        Key::Mouse(button) => match button {
            MouseButton::Left => u32::from(VK_LBUTTON.0),
            MouseButton::Right => u32::from(VK_RBUTTON.0),
            MouseButton::Middle => u32::from(VK_MBUTTON.0),
            MouseButton::Back => u32::from(VK_XBUTTON1.0),
            MouseButton::Forward => u32::from(VK_XBUTTON2.0),
        },
    }
}

/// The modifiers to pass to `RegisterHotKey`, always with `MOD_NOREPEAT` so that holding the
/// hotkey down does not repeat it.
pub fn register_modifiers(m: Modifiers) -> HOT_KEY_MODIFIERS {
    let mut flags = MOD_NOREPEAT;
    if m.ctrl {
        flags |= MOD_CONTROL;
    }
    if m.shift {
        flags |= MOD_SHIFT;
    }
    if m.alt {
        flags |= MOD_ALT;
    }
    flags
}

/// The virtual keys of `m`, to poll with `GetAsyncKeyState`. `VK_CONTROL` and `VK_MENU` cover
/// both the left and the right key, which is what the menu wants: any Ctrl or Alt counts.
pub fn modifier_vks(m: Modifiers) -> impl Iterator<Item = u32> {
    [
        (m.ctrl, VK_CONTROL.0),
        (m.shift, VK_SHIFT.0),
        (m.alt, VK_MENU.0),
    ]
    .into_iter()
    .filter_map(|(down, vk)| down.then_some(u32::from(vk)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_letters_and_digits_to_ascii() {
        assert_eq!(virtual_key(Key::Letter('A')), 0x41);
        assert_eq!(virtual_key(Key::Letter('Q')), 0x51);
        assert_eq!(virtual_key(Key::Letter('Z')), 0x5A);
        assert_eq!(virtual_key(Key::Digit(0)), 0x30);
        assert_eq!(virtual_key(Key::Digit(5)), 0x35);
        assert_eq!(virtual_key(Key::Digit(9)), 0x39);
    }

    #[test]
    fn maps_function_keys_from_vk_f1() {
        assert_eq!(virtual_key(Key::F(1)), 0x70);
        assert_eq!(virtual_key(Key::F(12)), 0x7B);
        assert_eq!(virtual_key(Key::F(24)), 0x87);
    }

    #[test]
    fn maps_space_and_backquote() {
        assert_eq!(virtual_key(Key::Space), 0x20);
        assert_eq!(virtual_key(Key::Backquote), 0xC0);
    }

    #[test]
    fn maps_mouse_buttons_to_their_virtual_keys() {
        assert_eq!(virtual_key(Key::Mouse(MouseButton::Left)), 0x01);
        assert_eq!(virtual_key(Key::Mouse(MouseButton::Right)), 0x02);
        assert_eq!(virtual_key(Key::Mouse(MouseButton::Middle)), 0x04);
        assert_eq!(virtual_key(Key::Mouse(MouseButton::Back)), 0x05);
        assert_eq!(virtual_key(Key::Mouse(MouseButton::Forward)), 0x06);
    }

    #[test]
    fn register_modifiers_are_flag_bits_with_norepeat() {
        assert_eq!(register_modifiers(Modifiers::default()).0, 0x4000);
        assert_eq!(
            register_modifiers(Modifiers {
                ctrl: true,
                ..Modifiers::default()
            })
            .0,
            0x4002
        );
        assert_eq!(
            register_modifiers(Modifiers {
                shift: true,
                ..Modifiers::default()
            })
            .0,
            0x4004
        );
        assert_eq!(
            register_modifiers(Modifiers {
                alt: true,
                ..Modifiers::default()
            })
            .0,
            0x4001
        );
        assert_eq!(
            register_modifiers(Modifiers {
                ctrl: true,
                shift: true,
                alt: true
            })
            .0,
            0x4007
        );
    }

    #[test]
    fn modifier_vks_are_the_polling_keys() {
        let none: Vec<u32> = modifier_vks(Modifiers::default()).collect();
        assert!(none.is_empty());

        let ctrl_shift: Vec<u32> = modifier_vks(Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        })
        .collect();
        assert_eq!(ctrl_shift, [0x11, 0x10]);

        let all: Vec<u32> = modifier_vks(Modifiers {
            ctrl: true,
            shift: true,
            alt: true,
        })
        .collect();
        assert_eq!(all, [0x11, 0x10, 0x12]);
    }
}
