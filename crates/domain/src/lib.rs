//! The model of radial-paste: snippets, settings, hotkey bindings and the radial menu's maths.
//! Pure Rust with no I/O and no Windows APIs, so all of it is unit-tested.
#![forbid(unsafe_code)]

mod config;
mod hotkey;
mod layout;

pub use config::{CONFIG_VERSION, Config, ConfigError, MAX_LABEL_CHARS, MAX_SNIPPETS, Snippet};
pub use hotkey::{HotkeyBinding, HotkeyError, Key, Modifiers, MouseButton};
pub use layout::{Layout, Rect, Vec2, place};
