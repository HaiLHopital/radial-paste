use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The modifier keys of a binding. The Windows key is left out on purpose: the shell owns most
/// of its combinations, and letting go of it opens the Start menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// A mouse button, usable as the hotkey's key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    /// The first side button, usually "back".
    Back,
    /// The second side button, usually "forward".
    Forward,
}

/// The key held down, with the modifiers, to open the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// `A` to `Z`.
    Letter(char),
    /// `0` to `9`.
    Digit(u8),
    /// `F1` to `F24`.
    F(u8),
    Space,
    /// The `` ` `` key.
    Backquote,
    /// A mouse button. The input adapter watches for its press; see `radial_win::input`.
    Mouse(MouseButton),
}

impl Key {
    fn is_valid(self) -> bool {
        match self {
            Key::Letter(c) => c.is_ascii_uppercase(),
            Key::Digit(d) => d <= 9,
            Key::F(n) => (1..=24).contains(&n),
            Key::Space | Key::Backquote | Key::Mouse(_) => true,
        }
    }

    /// A key that types something, or clicks something, needs Ctrl or Alt with it, or the hotkey
    /// would fire on ordinary typing or clicking. The middle, back and forward buttons carry no
    /// text and no ordinary click, so they may stand alone.
    fn needs_ctrl_or_alt(self) -> bool {
        match self {
            Key::Mouse(button) => matches!(button, MouseButton::Left | MouseButton::Right),
            Key::F(_) => false,
            Key::Letter(_) | Key::Digit(_) | Key::Space | Key::Backquote => true,
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Letter(c) => write!(f, "{c}"),
            Key::Digit(d) => write!(f, "{d}"),
            Key::F(n) => write!(f, "F{n}"),
            Key::Space => f.write_str("Space"),
            Key::Backquote => f.write_str("`"),
            Key::Mouse(button) => f.write_str(match button {
                MouseButton::Left => "MouseLeft",
                MouseButton::Right => "MouseRight",
                MouseButton::Middle => "MouseMiddle",
                MouseButton::Back => "MouseBack",
                MouseButton::Forward => "MouseForward",
            }),
        }
    }
}

impl FromStr for Key {
    type Err = HotkeyError;

    fn from_str(s: &str) -> Result<Self, HotkeyError> {
        let unknown = || HotkeyError::UnknownKey(s.to_owned());
        let upper = s.to_ascii_uppercase();
        let key = match upper.as_str() {
            "SPACE" => Key::Space,
            "`" | "BACKQUOTE" => Key::Backquote,
            "MOUSELEFT" => Key::Mouse(MouseButton::Left),
            "MOUSERIGHT" => Key::Mouse(MouseButton::Right),
            "MOUSEMIDDLE" => Key::Mouse(MouseButton::Middle),
            "MOUSEBACK" => Key::Mouse(MouseButton::Back),
            "MOUSEFORWARD" => Key::Mouse(MouseButton::Forward),
            _ => {
                let mut chars = upper.chars();
                match (chars.next(), chars.as_str()) {
                    (Some(c), "") if c.is_ascii_uppercase() => Key::Letter(c),
                    (Some(c), "") if c.is_ascii_digit() => Key::Digit(c as u8 - b'0'),
                    (Some('F'), number) => Key::F(number.parse().map_err(|_| unknown())?),
                    _ => return Err(unknown()),
                }
            }
        };
        if key.is_valid() {
            Ok(key)
        } else {
            Err(unknown())
        }
    }
}

/// A global hotkey, written like `Ctrl+Shift+Space`. Only valid bindings can be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HotkeyBinding {
    modifiers: Modifiers,
    key: Key,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum HotkeyError {
    #[error("unknown key `{0}`")]
    UnknownKey(String),
    #[error("`{0}` is written twice")]
    Repeated(String),
    #[error("a hotkey has one key besides Ctrl, Shift and Alt, not both `{0}` and `{1}`")]
    TwoKeys(String, String),
    #[error("a hotkey needs a key besides Ctrl, Shift and Alt")]
    MissingKey,
    #[error("the Windows key cannot be part of the hotkey")]
    WinKey,
    #[error("`{0}` needs Ctrl or Alt with it")]
    NeedsCtrlOrAlt(String),
}

impl HotkeyBinding {
    pub fn new(modifiers: Modifiers, key: Key) -> Result<Self, HotkeyError> {
        if !key.is_valid() {
            return Err(HotkeyError::UnknownKey(key.to_string()));
        }
        if key.needs_ctrl_or_alt() && !(modifiers.ctrl || modifiers.alt) {
            return Err(HotkeyError::NeedsCtrlOrAlt(key.to_string()));
        }
        Ok(Self { modifiers, key })
    }

    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    pub fn key(&self) -> Key {
        self.key
    }
}

impl Default for HotkeyBinding {
    /// `Ctrl+Shift+Space`: no Alt, and plain `Ctrl+Space` is taken by IDEs and input methods.
    fn default() -> Self {
        Self {
            modifiers: Modifiers {
                ctrl: true,
                shift: true,
                alt: false,
            },
            key: Key::Space,
        }
    }
}

impl fmt::Display for HotkeyBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Modifiers { ctrl, shift, alt } = self.modifiers;
        for (held, name) in [(ctrl, "Ctrl+"), (shift, "Shift+"), (alt, "Alt+")] {
            if held {
                f.write_str(name)?;
            }
        }
        write!(f, "{}", self.key)
    }
}

impl FromStr for HotkeyBinding {
    type Err = HotkeyError;

    /// Case-insensitive, with spaces allowed around the `+`.
    fn from_str(s: &str) -> Result<Self, HotkeyError> {
        let mut modifiers = Modifiers::default();
        let mut key: Option<(Key, &str)> = None;
        for part in s.split('+').map(str::trim) {
            let flag = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut modifiers.ctrl,
                "shift" => &mut modifiers.shift,
                "alt" => &mut modifiers.alt,
                "win" | "windows" | "super" | "meta" => return Err(HotkeyError::WinKey),
                _ => {
                    if let Some((_, first)) = key {
                        return Err(HotkeyError::TwoKeys(first.to_owned(), part.to_owned()));
                    }
                    key = Some((part.parse()?, part));
                    continue;
                }
            };
            if *flag {
                return Err(HotkeyError::Repeated(part.to_owned()));
            }
            *flag = true;
        }
        let (key, _) = key.ok_or(HotkeyError::MissingKey)?;
        Self::new(modifiers, key)
    }
}

impl TryFrom<String> for HotkeyBinding {
    type Error = HotkeyError;

    fn try_from(s: String) -> Result<Self, HotkeyError> {
        s.parse()
    }
}

impl From<HotkeyBinding> for String {
    fn from(binding: HotkeyBinding) -> String {
        binding.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<HotkeyBinding, HotkeyError> {
        s.parse()
    }

    #[test]
    fn round_trips_in_canonical_order() {
        let binding = parse(" shift + CTRL+space").unwrap();
        assert_eq!(binding, HotkeyBinding::default());
        assert_eq!(binding.to_string(), "Ctrl+Shift+Space");
        assert_eq!(parse("Alt+`").unwrap().to_string(), "Alt+`");
        assert_eq!(parse("ctrl+alt+7").unwrap().to_string(), "Ctrl+Alt+7");
    }

    #[test]
    fn function_keys_may_stand_alone() {
        assert_eq!(parse("F13").unwrap().key(), Key::F(13));
        assert_eq!(parse("f24").unwrap().key(), Key::F(24));
        assert_eq!(parse("F25"), Err(HotkeyError::UnknownKey("F25".into())));
        assert_eq!(parse("F0"), Err(HotkeyError::UnknownKey("F0".into())));
    }

    #[test]
    fn keys_that_type_need_ctrl_or_alt() {
        assert_eq!(parse("Q"), Err(HotkeyError::NeedsCtrlOrAlt("Q".into())));
        assert_eq!(
            parse("Shift+Space"),
            Err(HotkeyError::NeedsCtrlOrAlt("Space".into()))
        );
        assert!(parse("Ctrl+Q").is_ok());
    }

    #[test]
    fn mouse_buttons_parse_and_round_trip() {
        assert_eq!(
            parse("mouseback").unwrap().key(),
            Key::Mouse(MouseButton::Back)
        );
        assert_eq!(
            parse("Ctrl + MouseForward").unwrap().to_string(),
            "Ctrl+MouseForward"
        );
        assert_eq!(parse("MouseMiddle").unwrap().to_string(), "MouseMiddle");
        assert_eq!(
            parse("Mouse4"),
            Err(HotkeyError::UnknownKey("Mouse4".into()))
        );
    }

    #[test]
    fn middle_back_and_forward_may_stand_alone() {
        let none = Modifiers::default();
        for button in [MouseButton::Middle, MouseButton::Back, MouseButton::Forward] {
            assert!(HotkeyBinding::new(none, Key::Mouse(button)).is_ok());
        }
    }

    #[test]
    fn left_and_right_clicks_need_ctrl_or_alt() {
        assert_eq!(
            parse("MouseLeft"),
            Err(HotkeyError::NeedsCtrlOrAlt("MouseLeft".into()))
        );
        assert_eq!(
            parse("Shift+MouseRight"),
            Err(HotkeyError::NeedsCtrlOrAlt("MouseRight".into()))
        );
        assert!(parse("Ctrl+MouseLeft").is_ok());
    }

    #[test]
    fn rejects_malformed_bindings() {
        assert_eq!(parse("Win+Space"), Err(HotkeyError::WinKey));
        assert_eq!(parse("Ctrl+Shift"), Err(HotkeyError::MissingKey));
        assert_eq!(
            parse("Ctrl+ctrl+A"),
            Err(HotkeyError::Repeated("ctrl".into()))
        );
        assert_eq!(
            parse("Ctrl+A+B"),
            Err(HotkeyError::TwoKeys("A".into(), "B".into()))
        );
        assert_eq!(
            parse("Ctrl+Enter"),
            Err(HotkeyError::UnknownKey("Enter".into()))
        );
        assert_eq!(parse(""), Err(HotkeyError::UnknownKey("".into())));
    }

    #[test]
    fn new_rejects_keys_out_of_range() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert!(HotkeyBinding::new(ctrl, Key::Letter('a')).is_err());
        assert!(HotkeyBinding::new(ctrl, Key::Digit(10)).is_err());
    }
}
