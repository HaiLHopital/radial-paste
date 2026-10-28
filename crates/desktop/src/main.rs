//! radial-paste: hold a hotkey, flick toward a snippet, let go, and it is pasted.
//!
//! The composition root: it builds the adapters, starts the input thread and runs the
//! settings window on the main thread.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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
    // Before any window or coordinate exists: without per-monitor awareness the cursor and
    // monitor coordinates are virtualised and the ring lands in the wrong place.
    if !radial_win::dpi::enable_per_monitor_v2() {
        log::debug!("per-monitor DPI awareness was already set");
    }

    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,radial=info,wgpu_hal=off"),
    )
    .init();

    let repo = match TomlConfigRepository::at_default_location() {
        Ok(repo) => repo,
        Err(e) => {
            log::error!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let config = repo.load().unwrap_or_else(|e| {
        log::warn!("{e}; using the default settings");
        Config::default()
    });

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
    let result = radial_settings::run(config, move |config| {
        save_config(&repo, config).map_err(|e| e.to_string())?;
        save_handle.send(Event::ConfigChanged(config.clone()));
        Ok(())
    });
    input.quit();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    }
}
