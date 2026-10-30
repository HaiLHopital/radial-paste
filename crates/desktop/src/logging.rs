//! Logging: to the console while developing, to a file in release builds, where
//! `windows_subsystem = "windows"` hides the console.
//!
//! `env_logger` cannot write to files, so the release build carries a tiny logger of its own:
//! a filter built from `RUST_LOG` and a `Mutex<File>`, one file per run, the newest few kept.
// In debug builds only the console logger is installed, and the lints see just that build;
// the release machinery is allowed to sit unused there. Its tests still run in debug.
#![cfg_attr(debug_assertions, allow(dead_code))]

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{Log, Metadata, Record};

/// The same default as the console logger: this app's crates at info, the rest at warn.
const DEFAULT_FILTER: &str = "warn,radial=info";
/// How many run logs to keep in the logs folder.
const KEPT_LOGS: usize = 5;

/// Installs the logger for this build: the console in debug, a log file per run in release.
pub fn init() {
    #[cfg(debug_assertions)]
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(DEFAULT_FILTER))
        .init();

    #[cfg(not(debug_assertions))]
    {
        let mut builder = env_filter::Builder::new();
        builder.parse(&std::env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_FILTER.to_owned()));
        let filter = builder.build();
        log::set_max_level(filter.filter());
        match FileLogger::open(filter) {
            Ok(logger) => {
                let _ = log::set_boxed_logger(Box::new(logger));
            }
            Err(e) => {
                // No log file, and in release no console either: nothing is left to log into.
                eprintln!("could not open the log file: {e}");
            }
        }
    }
}

/// Appends to this run's file under `%LOCALAPPDATA%\radial-paste\logs`.
struct FileLogger {
    filter: env_filter::Filter,
    file: Mutex<File>,
}

impl FileLogger {
    fn open(filter: env_filter::Filter) -> std::io::Result<Self> {
        let dir = logs_dir()?;
        fs::create_dir_all(&dir)?;
        prune_old_logs(&dir);
        let name = format!("radial-paste-{}.log", file_stamp(now_secs()));
        let file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(dir.join(name))?;
        Ok(Self {
            filter,
            file: Mutex::new(file),
        })
    }
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.filter.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if !self.filter.matches(record) {
            return;
        }
        // A poisoned lock only means a line was half-written when a thread panicked; the later
        // lines should still go into the file, not be lost to the poison.
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(
            file,
            "{} {:<5} [{}] {}",
            line_stamp(now_secs(), subsec_millis()),
            record.level(),
            record.target(),
            record.args()
        );
    }

    fn flush(&self) {
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let _ = file.flush();
    }
}

/// `%LOCALAPPDATA%\radial-paste\logs`.
fn logs_dir() -> std::io::Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "LOCALAPPDATA is not set")
    })?;
    Ok(PathBuf::from(base).join("radial-paste").join("logs"))
}

/// Keeps only the newest [`KEPT_LOGS`] run logs; older ones go, best effort. The names carry a
/// timestamp, so sorting them sorts by age.
fn prune_old_logs(dir: &PathBuf) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
        .collect();
    logs.sort();
    while logs.len() > KEPT_LOGS
        && let Some(oldest) = logs.first()
    {
        let _ = fs::remove_file(oldest);
        logs.remove(0);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn subsec_millis() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_millis()
}

/// The UTC calendar date of `days` since the epoch: Hinnant's `civil_from_days`, so that the
/// logger needs no date dependency.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// For log lines: `2026-10-28T16:00:00.042Z`.
fn line_stamp(secs: u64, millis: u32) -> String {
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let tod = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        tod / 3_600,
        tod % 3_600 / 60,
        tod % 60
    )
}

/// For file names, where `:` is not allowed: `20261028-160000`.
fn file_stamp(secs: u64) -> String {
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let tod = secs % 86_400;
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        tod / 3_600,
        tod % 3_600 / 60,
        tod % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_is_the_first_of_january_1970() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(line_stamp(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(file_stamp(0), "19700101-000000");
    }

    #[test]
    fn a_known_day_comes_out_right() {
        // 2000-03-01 is day 11017 of the epoch, from Hinnant's own test suite.
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        // 2026-10-28T16:00:00Z is 20 754 days plus 16 hours.
        assert_eq!(
            line_stamp(20_754 * 86_400 + 16 * 3_600, 42),
            "2026-10-28T16:00:00.042Z"
        );
        assert_eq!(file_stamp(20_754 * 86_400 + 16 * 3_600), "20261028-160000");
    }
}
