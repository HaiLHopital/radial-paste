//! The traits adapters implement. Events flowing in are [`crate::Event`]s, not traits.

use std::fmt::Display;

use radial_domain::{Config, HotkeyBinding, Layout, Snippet, Vec2};

/// Where the settings live.
pub trait ConfigRepository {
    type Error: std::error::Error;

    fn load(&self) -> Result<Config, Self::Error>;
    fn save(&self, config: &Config) -> Result<(), Self::Error>;
}

/// Where the menu was drawn, in physical screen pixels, and how they map to the `Layout`'s
/// logical (96-DPI) ones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The centre the menu actually used: near a screen edge it moves inward to stay whole.
    pub centre: Vec2,
    /// The monitor's DPI over 96. Offsets from `centre` are physical pixels; divide by this
    /// before `Layout::pick` and `contains`.
    pub scale: f64,
}

/// Draws the radial menu. The cursor is in physical screen pixels, the `Layout` in logical
/// (96-DPI) ones.
pub trait MenuPresenter {
    /// Shows the menu around `cursor` with nothing highlighted, and returns where it put it:
    /// near a screen edge the menu moves inward to stay whole, and its monitor gives the scale.
    fn show(&mut self, cursor: Vec2, snippets: &[Snippet], layout: Layout) -> Placement;
    /// Highlights a wedge, or none. Called only when the highlight changes.
    fn highlight(&mut self, wedge: Option<usize>);
    fn hide(&mut self);
}

/// Puts text into whichever window has the focus.
pub trait TextInjector {
    type Error: Display;

    fn paste(&mut self, text: &str) -> Result<(), Self::Error>;
}

/// Owns the global hotkey.
pub trait HotkeyRegistrar {
    type Error: Display;

    /// Replaces the registered hotkey. On failure the previous one stays registered.
    fn register(&mut self, binding: HotkeyBinding) -> Result<(), Self::Error>;

    /// Registers Esc while the menu is up, so that it can close the menu. The menu's window
    /// never has the focus, so Esc cannot be read from a window. Registrars that cannot take a
    /// second hotkey may ignore this.
    fn register_escape(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Lets Esc go again. Called when the menu closes.
    fn unregister_escape(&mut self) {}
}
