//! Windows adapters for the ports in `radial_app::ports`, and the input thread.
//!
//! Every adapter is real. `inject` still needs its manual test matrix (terminals and elevated
//! windows are known not to work).

use windows::Win32::Foundation::GetLastError;
use windows::core::{Error as WinError, HRESULT};

pub mod dpi;
pub mod hotkey;
pub mod inject;
pub mod input;
pub mod keys;
pub mod mouse;
pub mod overlay;

/// The calling thread's last Win32 error, as a typed error.
pub(crate) fn last_error() -> WinError {
    // SAFETY: just reads the thread's last error code.
    WinError::from_hresult(HRESULT::from_win32(unsafe { GetLastError() }.0))
}
