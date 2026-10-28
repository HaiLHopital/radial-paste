//! Windows adapters for the ports in `radial_app::ports`, and the input thread.
//!
//! `keys`, `hotkey`, `input`, `overlay` and `dpi` are real. `inject` is still a stand-in that
//! logs what it would do; its module doc says which Win32 calls will replace it.

pub mod dpi;
pub mod hotkey;
pub mod inject;
pub mod input;
pub mod keys;
pub mod overlay;
