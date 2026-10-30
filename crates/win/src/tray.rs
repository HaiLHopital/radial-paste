//! The tray icon: the resident's only face while it runs in the background.
//!
//! The tray must be created on a thread that then keeps pumping messages, which is the host
//! window's loop (`crate::host`): the clicks arrive as ordinary window messages, and this
//! module's channels fill while the loop dispatches them. [`Tray::drain`] turns them into
//! commands for the resident.

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

/// What one pump of the tray's event channels produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// The user asked for the settings window, by menu or by double-click.
    ShowSettings,
    /// The Pause checkbox flipped; the argument is its new state.
    Pause(bool),
    /// The user asked the resident to end.
    Quit,
}

#[derive(Debug, thiserror::Error)]
pub enum TrayError {
    #[error("could not load the tray icon: {0}")]
    Icon(tray_icon::BadIcon),
    #[error("could not build the tray menu: {0}")]
    Menu(tray_icon::menu::Error),
    #[error("could not create the tray icon: {0}")]
    Create(tray_icon::Error),
}

/// The tray icon with its menu. Keep it alive: dropping it removes the icon.
pub struct Tray {
    _icon: TrayIcon,
    pause: CheckMenuItem,
}

impl Tray {
    /// Creates the icon and its menu. Must run on the thread that owns the message loop.
    pub fn new() -> Result<Self, TrayError> {
        let icon = match tray_icon::Icon::from_resource(1, None) {
            Ok(icon) => icon,
            Err(resource_error) => match fallback_icon() {
                Some(icon) => icon,
                None => return Err(TrayError::Icon(resource_error)),
            },
        };
        let settings = MenuItem::with_id("settings", "Settings…", true, None);
        let pause = CheckMenuItem::with_id("pause", "Pause", true, false, None);
        let quit = MenuItem::with_id("quit", "Quit", true, None);
        let menu = Menu::new();
        menu.append(&settings).map_err(TrayError::Menu)?;
        menu.append(&pause).map_err(TrayError::Menu)?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(TrayError::Menu)?;
        menu.append(&quit).map_err(TrayError::Menu)?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("radial-paste")
            .with_icon(icon)
            // The menu belongs to the right button; the left button stays free, so that a
            // double-click means the settings window and nothing else.
            .with_menu_on_left_click(false)
            .build()
            .map_err(TrayError::Create)?;
        Ok(Self { _icon: icon, pause })
    }

    /// Drains both event channels. Call it once per host-loop iteration; the channels only
    /// fill while the loop dispatches messages.
    pub fn drain(&self) -> Vec<Command> {
        let mut commands = Vec::new();
        for event in MenuEvent::receiver().try_iter() {
            // The checkbox has already flipped by the time the event arrives, so what the
            // resident wants is the new state, not a toggle.
            let command = match event.id.as_ref() {
                "settings" => Some(Command::ShowSettings),
                "pause" => Some(Command::Pause(self.pause.is_checked())),
                "quit" => Some(Command::Quit),
                _ => None,
            };
            commands.extend(command);
        }
        for event in TrayIconEvent::receiver().try_iter() {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                commands.push(Command::ShowSettings);
            }
        }
        commands
    }
}

/// A tiny version of the app icon, drawn by hand, for the case where the embedded resource is
/// missing: a dark disc with an accent centre.
fn fallback_icon() -> Option<tray_icon::Icon> {
    const SIZE: u32 = 32;
    let centre = SIZE as f32 / 2.0;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 + 0.5 - centre;
            let dy = y as f32 + 0.5 - centre;
            let (r, g, b, a) = if (dx * dx + dy * dy).sqrt() <= centre - 1.0 {
                (32, 34, 40, 255)
            } else {
                (0, 0, 0, 0)
            };
            rgba.extend([r, g, b, a]);
        }
    }
    tray_icon::Icon::from_rgba(rgba, SIZE, SIZE).ok()
}
