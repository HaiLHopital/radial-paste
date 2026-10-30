//! radial-paste: hold a hotkey, flick toward a snippet, let go, and it is pasted.
//!
//! The composition root: it builds the adapters, starts the input thread and runs the
//! settings window on the main thread.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod logging;

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use radial_app::ports::ConfigRepository;
use radial_app::{Core, Event, save_config};
use radial_domain::Config;
use radial_store::TomlConfigRepository;
use radial_win::hotkey::Hotkeys;
use radial_win::inject::ClipboardPaste;
use radial_win::input;
use radial_win::overlay::Overlay;

fn main() -> ExitCode {
    // Logging first, so that the next line's outcome is not lost.
    logging::init();

    // Before any window or coordinate exists: without per-monitor awareness the cursor and
    // monitor coordinates are virtualised and the ring lands in the wrong place. With the
    // manifest's PerMonitorV2 this only logs that it was already set.
    if !radial_win::dpi::enable_per_monitor_v2() {
        log::debug!("per-monitor DPI awareness was already set");
    }

    let repo = match TomlConfigRepository::at_default_location() {
        Ok(repo) => repo,
        Err(e) => {
            log::error!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let config = match repo.load() {
        Ok(config) => config,
        Err(e) => {
            log::error!("{e}; using the default settings");
            back_up_bad_config(repo.path());
            Config::default()
        }
    };

    let input = match input::spawn(Core::new(
        config.clone(),
        Overlay::default(),
        ClipboardPaste,
        Hotkeys::default(),
    )) {
        Ok(input) => input,
        Err(e) => {
            log::error!("{e}");
            return ExitCode::FAILURE;
        }
    };

    // Until there is a tray icon, the settings window is the app: closing it ends the input
    // thread, and with it the process.
    let save_handle = input.clone();
    let result = radial_settings::run(
        config,
        move |config| {
            save_config(&repo, config).map_err(|e| e.to_string())?;
            save_handle.send(Event::ConfigChanged(config.clone()));
            Ok(())
        },
        || {
            radial_win::mouse::pressed_button()
                .map(|button| (button, radial_win::mouse::modifiers_down()))
        },
    );
    input.quit();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Keeps a copy of a config that failed to load, so that the user can recover or diff it
/// instead of retyping it, before the defaults take over.
fn back_up_bad_config(path: &Path) {
    let backup = path.with_extension("toml.bad");
    match fs::copy(path, &backup) {
        Ok(_) => log::info!("the broken config was kept as {}", backup.display()),
        Err(e) => log::warn!("could not keep a copy of the broken config: {e}"),
    }
}
