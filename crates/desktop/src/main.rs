//! radial-paste: hold a hotkey, flick toward a snippet, let go, and it is pasted.
//!
//! The composition root. With no arguments this process is the resident: it starts the input
//! thread, shows the tray icon and runs the host window's message loop until Quit. With
//! `--settings` it opens the settings window instead, saves the config and asks a running
//! resident to reload it - eframe's event loop cannot be restarted inside one process
//! (open decision 1), so the two live in separate processes.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod logging;

use std::path::Path;
use std::process::ExitCode;

use radial_app::ports::ConfigRepository;
use radial_app::{Core, Event, save_config};
use radial_domain::Config;
use radial_store::TomlConfigRepository;
use radial_win::autostart;
use radial_win::host::{self, Flow, HostMessage, InstanceError};
use radial_win::hotkey::Hotkeys;
use radial_win::inject::ClipboardPaste;
use radial_win::input::{self, InputHandle};
use radial_win::overlay::Overlay;
use radial_win::tray::{Command, Tray};

fn main() -> ExitCode {
    // Logging first, so that the next line's outcome is not lost.
    logging::init();

    // Before any window or coordinate exists: without per-monitor awareness the cursor and
    // monitor coordinates are virtualised and the ring lands in the wrong place. With the
    // manifest's PerMonitorV2 this only logs that it was already set.
    if !radial_win::dpi::enable_per_monitor_v2() {
        log::debug!("per-monitor DPI awareness was already set");
    }

    if std::env::args().any(|arg| arg == "--settings") {
        settings_mode()
    } else {
        resident_mode()
    }
}

/// The settings window, in its own process. Saving writes the file and asks the resident to
/// reload it; with no resident running, only the file is written.
fn settings_mode() -> ExitCode {
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
    let result = radial_settings::run(
        config,
        move |config| {
            save_config(&repo, config).map_err(|e| e.to_string())?;
            if host::notify_reload() {
                log::debug!("asked the resident to reload the settings");
            } else {
                log::debug!("no resident is running; only the file is written");
            }
            Ok(())
        },
        || {
            radial_win::mouse::pressed_button()
                .map(|button| (button, radial_win::mouse::modifiers_down()))
        },
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// The resident: hotkey, overlay, tray and the host window's loop, all in the background and
/// without a window of its own. Ends when the tray's Quit is used.
fn resident_mode() -> ExitCode {
    match host::try_acquire() {
        Ok(instance) => instance,
        Err(InstanceError::AlreadyRunning) => {
            log::info!("radial-paste is already running; opening its settings");
            host::notify_open_settings();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            log::error!("{e}");
            return ExitCode::FAILURE;
        }
    };

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
    apply_autostart(config.autostart);

    let input = match input::spawn(Core::new(
        config,
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

    // Without the tray the resident would be an unclosable ghost; it failing is logged, but
    // the hotkey still works.
    let tray = match Tray::new() {
        Ok(tray) => Some(tray),
        Err(e) => {
            log::warn!("could not create the tray icon: {e}");
            None
        }
    };

    let reload_repo = TomlConfigRepository::at(repo.path());
    let reload_input = input.clone();
    let on_message = move |message| match message {
        HostMessage::Reload => reload(&reload_repo, &reload_input),
        HostMessage::OpenSettings => open_settings(),
    };
    let on_pump = move || {
        if let Some(tray) = &tray {
            for command in tray.drain() {
                match command {
                    Command::ShowSettings => open_settings(),
                    Command::Pause(paused) => input.send(Event::SetPaused(paused)),
                    Command::Quit => {
                        input.quit();
                        return Flow::Quit;
                    }
                }
            }
        }
        Flow::Continue
    };

    match host::run_host(on_message, on_pump) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Re-reads the config and puts it in effect. A file that fails to load does not throw the
/// running settings away; the next save or reload can fix it.
fn reload(repo: &TomlConfigRepository, input: &InputHandle) {
    match repo.load() {
        Ok(config) => {
            apply_autostart(config.autostart);
            input.send(Event::ConfigChanged(config));
        }
        Err(e) => log::error!("{e}; keeping the settings that are in effect"),
    }
}

/// Starts the settings window in its own process.
fn open_settings() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            log::error!("could not locate the executable: {e}");
            return;
        }
    };
    if let Err(e) = std::process::Command::new(exe).arg("--settings").spawn() {
        log::error!("could not start the settings window: {e}");
    }
}

/// Applies the `autostart` setting to the registry. Failures are logged, not fatal: the
/// setting only takes effect again on the next start or reload.
fn apply_autostart(enabled: bool) {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            log::warn!("could not locate the executable: {e}");
            return;
        }
    };
    if let Err(e) = autostart::set(enabled, &exe) {
        log::warn!("{e}");
    }
}

/// Keeps a copy of a config that failed to load, so that the user can recover or diff it
/// instead of retyping it, before the defaults take over.
fn back_up_bad_config(path: &Path) {
    let backup = path.with_extension("toml.bad");
    match std::fs::copy(path, &backup) {
        Ok(_) => log::info!("the broken config was kept as {}", backup.display()),
        Err(e) => log::warn!("could not keep a copy of the broken config: {e}"),
    }
}
