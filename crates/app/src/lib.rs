//! What radial-paste does, independent of Windows: the menu's state machine ([`Core`]) and
//! saving settings. It reaches the outside world only through the traits in [`ports`].
#![forbid(unsafe_code)]

mod machine;
pub mod ports;
mod settings;

pub use machine::{Core, CoreError, Event};
pub use settings::{SaveError, save_config};
