//! Pasting into the focused window.
//!
//! Planned, by clipboard:
//! 1. Save the clipboard's current contents, in every format.
//! 2. Put the text on it as `CF_UNICODETEXT`, together with the
//!    `ExcludeClipboardContentFromMonitorProcessing` format so it stays out of Win+V history.
//! 3. `SendInput`: Ctrl down, V down, V up, Ctrl up.
//! 4. About 100 ms later, put the saved contents back.
//!
//! An elevated target window ignores input from a normal process (UIPI). That fails silently.

use std::convert::Infallible;

use radial_app::ports::TextInjector;

#[derive(Default)]
pub struct ClipboardPaste;

impl TextInjector for ClipboardPaste {
    type Error = Infallible;

    fn paste(&mut self, text: &str) -> Result<(), Infallible> {
        log::info!("stub: would paste {} characters", text.chars().count());
        Ok(())
    }
}
