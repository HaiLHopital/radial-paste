//! The `autostart` setting, applied to the registry's per-user Run key, which Windows walks at
//! login. The entry points at the running executable, so moving the app and toggling the
//! setting again re-points it.

use std::path::Path;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
    RegSetValueExW,
};
use windows::core::{Error as WinError, HRESULT, PCWSTR};

/// The per-user Run key, whose values are started at login.
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// The value's name, recognizable next to the other entries.
const VALUE_NAME: &str = "radial-paste";

/// An error from the Run key, naming the operation that failed.
#[derive(Debug, thiserror::Error)]
pub enum AutostartError {
    #[error("could not open the registry Run key: {0}")]
    Open(WinError),
    #[error("could not write the autostart entry: {0}")]
    Set(WinError),
    #[error("could not remove the autostart entry: {0}")]
    Remove(WinError),
}

/// A `WIN32_ERROR` as a typed error, because the registry calls report through their return
/// value instead of the thread's last error.
fn win32_error(error: WIN32_ERROR) -> WinError {
    WinError::from_hresult(HRESULT::from_win32(error.0))
}

/// The command line the Run key carries: quoted, because paths with spaces would otherwise
/// break apart.
fn command_line(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Points the Run key at `exe`, or removes the entry again. A missing entry is exactly what
/// `false` wants, so removing one never fails on that account.
pub fn set(enabled: bool, exe: &Path) -> Result<(), AutostartError> {
    let wide_key = wide(RUN_KEY);
    let mut key = HKEY::default();
    // SAFETY: `key` is an out parameter; the handle it receives is closed below.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(wide_key.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut key,
        )
    };
    if opened != ERROR_SUCCESS {
        return Err(AutostartError::Open(win32_error(opened)));
    }

    let outcome = if enabled {
        let mut value = command_line(exe).encode_utf16().collect::<Vec<_>>();
        value.push(0);
        let bytes = value
            .iter()
            .flat_map(|unit| unit.to_ne_bytes())
            .collect::<Vec<u8>>();
        let wide_name = wide(VALUE_NAME);
        // SAFETY: the bytes are a NUL-terminated UTF-16 string, which is what `REG_SZ` wants,
        // and the key is the one opened above.
        let written = unsafe {
            RegSetValueExW(
                key,
                PCWSTR::from_raw(wide_name.as_ptr()),
                None,
                REG_SZ,
                Some(&bytes),
            )
        };
        if written == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(AutostartError::Set(win32_error(written)))
        }
    } else {
        let wide_name = wide(VALUE_NAME);
        // SAFETY: the key is the one opened above; the call only names a value to remove.
        let deleted = unsafe { RegDeleteValueW(key, PCWSTR::from_raw(wide_name.as_ptr())) };
        if deleted == ERROR_SUCCESS || deleted == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(AutostartError::Remove(win32_error(deleted)))
        }
    };

    // SAFETY: the handle came from the successful open above and is used by nothing else.
    let closed = unsafe { RegCloseKey(key) };
    if closed != ERROR_SUCCESS {
        log::warn!(
            "could not close the registry Run key: {}",
            win32_error(closed)
        );
    }
    outcome
}

/// The string as a NUL-terminated UTF-16 buffer.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::{KEY_QUERY_VALUE, REG_VALUE_TYPE, RegQueryValueExW};

    #[test]
    fn the_command_line_is_quoted() {
        assert_eq!(
            command_line(Path::new("C:\\tools\\radial-paste.exe")),
            "\"C:\\tools\\radial-paste.exe\""
        );
        assert_eq!(
            command_line(Path::new(
                "C:\\Program Files\\radial-paste\\radial-paste.exe"
            )),
            "\"C:\\Program Files\\radial-paste\\radial-paste.exe\""
        );
    }

    /// This one touches the real per-user Run key: it writes the entry under the value name
    /// the app uses and removes it again, leaving the key as it was. Worth more than a fake
    /// registry, because the whole point is that Windows reads this exact spot at login.
    #[test]
    fn set_and_unset_write_the_real_run_key() {
        let exe = Path::new("C:\\some day\\radial-paste.exe");

        set(true, exe).unwrap();
        let written = read_value().unwrap();
        assert_eq!(written.as_deref(), Some(command_line(exe).as_str()));

        set(false, exe).unwrap();
        assert_eq!(read_value().unwrap(), None);

        // Removing a missing entry is not an error, and leaves nothing behind.
        set(false, exe).unwrap();
        assert_eq!(read_value().unwrap(), None);
    }

    /// The value as it sits in the key, read back with the same calls `set` uses, or `None`
    /// when it is not there.
    fn read_value() -> Result<Option<String>, AutostartError> {
        let wide_key = wide(RUN_KEY);
        let mut key = HKEY::default();
        // SAFETY: `key` is an out parameter; the handle is closed before returning.
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR::from_raw(wide_key.as_ptr()),
                None,
                KEY_QUERY_VALUE,
                &mut key,
            )
        };
        if opened != ERROR_SUCCESS {
            return Err(AutostartError::Open(win32_error(opened)));
        }

        let mut kind = REG_VALUE_TYPE::default();
        let mut bytes = [0u8; 1024];
        let mut len = bytes.len() as u32;
        let wide_name = wide(VALUE_NAME);
        // SAFETY: the buffers are ours, and the key is the one opened above.
        let read = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR::from_raw(wide_name.as_ptr()),
                None,
                Some(&mut kind),
                Some(bytes.as_mut_ptr()),
                Some(&mut len),
            )
        };
        // SAFETY: the handle came from the successful open above.
        let _ = unsafe { RegCloseKey(key) };

        if read == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if read != ERROR_SUCCESS {
            return Err(AutostartError::Remove(win32_error(read)));
        }
        assert_eq!(
            kind, REG_SZ,
            "the test wrote the value itself; it is a string"
        );
        assert_eq!(
            kind, REG_SZ,
            "the test wrote the value itself; it is a string"
        );
        assert_eq!(len % 2, 0, "UTF-16 comes in whole units");
        // `len` includes the NUL terminator the registry appends; decode only the units.
        let units = &bytes[..len as usize / 2 * 2];
        let text: Vec<u16> = units
            .chunks(2)
            .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
            .collect();
        let text = String::from_utf16(&text).unwrap();
        Ok(Some(text.trim_end_matches('\0').to_owned()))
    }
}
