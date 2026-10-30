//! Pasting into the focused window, by the clipboard.
//!
//! The clipboard carries the text: its current formats are saved, the snippet is put on it as
//! `CF_UNICODETEXT` together with the registered formats that keep it out of Win+V history and
//! the cloud clipboard, and it is closed again so that the target can read it. `SendInput`
//! types Ctrl+V. A moment later the saved formats go back - unless the clipboard's sequence
//! number shows that something else used it in the meantime.
//!
//! An elevated target window ignores input from a normal process (UIPI), and Windows reports no
//! error for it, so that failure is silent. Pasting into terminals needs Ctrl+Shift+V and does
//! not work yet.

use std::mem::size_of;
use std::time::Duration;

use radial_app::ports::TextInjector;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{
    CF_BITMAP, CF_ENHMETAFILE, CF_METAFILEPICT, CF_OWNERDISPLAY, CF_PALETTE, CF_UNICODETEXT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_CONTROL,
};
use windows::core::{Error as WinError, HSTRING, PCWSTR};

/// How long the target gets to read the clipboard before the old contents go back.
const READ_DELAY: Duration = Duration::from_millis(100);
/// The retries give a clipboard held by another app about 100 ms in total.
const BUSY_WAIT: Duration = Duration::from_millis(10);
const BUSY_ATTEMPTS: u32 = 10;
/// How often the replacement is tried. `EmptyClipboard` and `SetClipboardData` question the
/// clipboard's previous owner synchronously; if that owner is busy - clipboard managers like to
/// rewrite everything they see - the call fails without a real error code, and a moment later
/// it works again.
const STAGE_ATTEMPTS: u32 = 3;
const STAGE_RETRY_WAIT: Duration = Duration::from_millis(50);

#[derive(Debug, thiserror::Error)]
pub enum PasteError {
    #[error("the clipboard stayed busy; another app would not let it go")]
    Busy,
    #[error("could not clear the clipboard: {0}")]
    Clear(WinError),
    #[error("could not allocate memory for clipboard format {format}: {source}")]
    Alloc { format: u32, source: WinError },
    #[error("could not lock the memory for clipboard format {format}")]
    Lock { format: u32 },
    #[error("could not set clipboard format {format}: {source}")]
    SetFormat { format: u32, source: WinError },
    #[error("could not register the clipboard format {name:?}")]
    Register { name: String },
    #[error("could not send the paste: only {sent} of 4 keystrokes went in: {source}")]
    SendInput { sent: u32, source: WinError },
}

#[derive(Debug, Default)]
pub struct ClipboardPaste;

impl TextInjector for ClipboardPaste {
    type Error = PasteError;

    fn paste(&mut self, text: &str) -> Result<(), Self::Error> {
        // The user's formats from the first attempt are the ones worth restoring: later
        // attempts may see a clipboard that an earlier attempt already cleared.
        let mut carried: Option<SavedFormats> = None;
        let mut attempts_left = STAGE_ATTEMPTS;
        loop {
            match stage_replacement(text) {
                Ok((mut saved, sequence)) => {
                    if let Some(original) = carried.take() {
                        saved = original;
                    }
                    return send_then_restore(saved, sequence);
                }
                Err(staged) => {
                    carried = carried.or(staged.saved);
                    attempts_left -= 1;
                    if attempts_left == 0 {
                        if let Some(saved) = carried
                            && let Err(e) = restore_formats(saved, None)
                        {
                            log::warn!(
                                "could not restore the clipboard after the failed paste: {e}"
                            );
                        }
                        return Err(staged.error);
                    }
                    std::thread::sleep(STAGE_RETRY_WAIT);
                }
            }
        }
    }
}

/// A clipboard error together with the formats that were saved before it hit, so that a failed
/// paste can still undo its half-done replacement. `saved` is `None` when the error struck
/// before anything was changed, in which case there is nothing to undo.
struct StagedError {
    error: PasteError,
    saved: Option<SavedFormats>,
}

/// A clipboard snapshot: format id and its bytes.
type SavedFormats = Vec<(u32, Vec<u8>)>;

/// Saves the old formats, replaces the clipboard with the snippet and its exclusion formats,
/// and returns both. The clipboard is closed again by the time this returns, so that the target
/// can read it.
fn stage_replacement(text: &str) -> Result<(SavedFormats, u32), StagedError> {
    let _clipboard = open_clipboard().map_err(|error| StagedError { error, saved: None })?;
    let saved = save_formats();

    match write_replacement(text) {
        Ok(sequence) => Ok((saved, sequence)),
        Err(error) => Err(StagedError {
            error,
            saved: Some(saved),
        }),
    }
}

/// Clears the clipboard and writes the snippet and the exclusion formats, returning the
/// sequence number right after.
fn write_replacement(text: &str) -> Result<u32, PasteError> {
    // SAFETY: the clipboard is open on this thread, and the guard in `stage_replacement`
    // closes it again.
    unsafe { EmptyClipboard() }.map_err(PasteError::Clear)?;
    set_text(text)?;
    exclude_from_history()?;
    // SAFETY: no preconditions; this only reads the session-wide counter.
    Ok(unsafe { GetClipboardSequenceNumber() })
}

/// Sends Ctrl+V and, once the target has had a moment to read, restores the saved formats. A
/// failed send restores too: the clipboard still carries the snippet, and no target got it.
fn send_then_restore(saved: SavedFormats, sequence: u32) -> Result<(), PasteError> {
    if let Err(e) = send_ctrl_v() {
        if let Err(r) = restore_formats(saved, Some(sequence)) {
            log::warn!("could not restore the clipboard after the failed paste: {r}");
        }
        return Err(e);
    }
    std::thread::sleep(READ_DELAY);
    // SAFETY: no preconditions; this only reads the session-wide counter.
    let current = unsafe { GetClipboardSequenceNumber() };
    if current != sequence {
        log::debug!(
            "the clipboard changed during the paste ({sequence} -> {current}); leaving it alone"
        );
        return Ok(());
    }
    if let Err(e) = restore_formats(saved, Some(sequence)) {
        // The paste itself worked; a failed restore only costs the user their copy.
        log::warn!("could not restore the clipboard: {e}");
    }
    Ok(())
}

/// A token proving that the clipboard is open on this thread; closes it on drop, so that every
/// early return leaves the clipboard usable again.
struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: the clipboard was opened by `open_clipboard` on this same thread.
        let _ = unsafe { CloseClipboard() };
    }
}

/// Opens the clipboard, retrying while another app holds it.
fn open_clipboard() -> Result<ClipboardGuard, PasteError> {
    for attempt in 0..BUSY_ATTEMPTS {
        // SAFETY: with no window the clipboard is tied to this task; the guard closes it again.
        if unsafe { OpenClipboard(None) }.is_ok() {
            return Ok(ClipboardGuard);
        }
        if attempt + 1 < BUSY_ATTEMPTS {
            std::thread::sleep(BUSY_WAIT);
        }
    }
    Err(PasteError::Busy)
}

/// Every `HGLOBAL`-backed format with its bytes. The GDI-handle formats are skipped (open
/// decision 5): their handles cannot be copied as bytes, and images come back through the saved
/// `CF_DIB` instead.
fn save_formats() -> SavedFormats {
    const SKIPPED: [u32; 8] = [
        CF_BITMAP.0 as u32,
        CF_METAFILEPICT.0 as u32,
        CF_PALETTE.0 as u32,
        CF_ENHMETAFILE.0 as u32,
        CF_OWNERDISPLAY.0 as u32,
        // The `CF_DSP*` display variants hold GDI handles as well, but they are not in the
        // `windows` crate, so they are listed by value.
        0x0082, // CF_DSPBITMAP
        0x0083, // CF_DSPMETAFILEPICT
        0x008E, // CF_DSPENHMETAFILE
    ];
    let mut saved = Vec::new();
    let mut format = 0;
    loop {
        // SAFETY: 0 starts the enumeration; each call returns the next format, or 0 at the end.
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            return saved;
        }
        if SKIPPED.contains(&format) {
            continue;
        }
        // SAFETY: the clipboard is open on this thread and the format came from the enumeration.
        let Ok(handle) = (unsafe { GetClipboardData(format) }) else {
            continue;
        };
        if handle.0.is_null() {
            continue;
        }
        let block = HGLOBAL(handle.0);
        // SAFETY: the block is alive while the clipboard is open; the lock is released below.
        let ptr = unsafe { GlobalLock(block) };
        if ptr.is_null() {
            continue;
        }
        // The block can be larger than the data; trailing bytes are harmless, because readers
        // stop at the data's own terminator.
        let size = unsafe { GlobalSize(block) };
        let mut data = vec![0u8; size];
        // SAFETY: `GlobalSize` is the block's size, so the copy stays in bounds.
        unsafe { std::ptr::copy_nonoverlapping(ptr.cast::<u8>(), data.as_mut_ptr(), size) };
        // SAFETY: matching unlock for the lock above.
        let _ = unsafe { GlobalUnlock(block) };
        saved.push((format, data));
    }
}

fn set_text(text: &str) -> Result<(), PasteError> {
    let units = to_utf16_with_crlf(text);
    let mut bytes = Vec::with_capacity(units.len() * 2);
    for unit in units {
        bytes.extend_from_slice(&unit.to_ne_bytes());
    }
    put_format(CF_UNICODETEXT.0 as u32, &bytes)
}

/// The three registered formats that keep the pasted text out of Win+V history and the cloud
/// clipboard: an empty marker and two DWORD zeros.
fn exclude_from_history() -> Result<(), PasteError> {
    let no_monitor = register_format("ExcludeClipboardContentFromMonitorProcessing")?;
    let no_history = register_format("CanIncludeInClipboardHistory")?;
    let no_cloud = register_format("CanUploadToCloudClipboard")?;
    put_format(no_monitor, &[])?;
    put_format(no_history, &0u32.to_ne_bytes())?;
    put_format(no_cloud, &0u32.to_ne_bytes())
}

fn register_format(name: &str) -> Result<u32, PasteError> {
    let wide = HSTRING::from(name);
    // SAFETY: the string lives through the call, which only reads it.
    let format = unsafe { RegisterClipboardFormatW(PCWSTR::from_raw(wide.as_ptr())) };
    if format == 0 {
        return Err(PasteError::Register {
            name: name.to_owned(),
        });
    }
    Ok(format)
}

/// Puts `bytes` on the clipboard as `format`, in a moveable global that the system then owns.
fn put_format(format: u32, bytes: &[u8]) -> Result<(), PasteError> {
    // A zero-size allocation comes back as a discarded handle, which `SetClipboardData` rejects
    // - with the stale "completed successfully" error - so even empty formats carry one zero
    // byte. Their readers only care that the format is there.
    let bytes = if bytes.is_empty() { &[0u8][..] } else { bytes };
    // SAFETY: a fresh moveable allocation of exactly `bytes.len()` bytes.
    let block = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }
        .map_err(|source| PasteError::Alloc { format, source })?;
    // SAFETY: locking our own fresh allocation; the lock is released right below.
    let ptr = unsafe { GlobalLock(block) };
    if ptr.is_null() {
        // SAFETY: our own allocation, not handed to the system.
        let _ = unsafe { GlobalFree(Some(block)) };
        return Err(PasteError::Lock { format });
    }
    // SAFETY: the block is `bytes.len()` bytes long.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast(), bytes.len());
        let _ = GlobalUnlock(block);
    }
    // SAFETY: the system takes ownership on success; on failure the block is freed instead.
    match unsafe { SetClipboardData(format, Some(HANDLE(block.0))) } {
        Ok(_) => Ok(()),
        Err(source) => {
            let _ = unsafe { GlobalFree(Some(block)) };
            Err(PasteError::SetFormat { format, source })
        }
    }
}

/// Types Ctrl+V with `SendInput`. Fewer than four inserted events is an error; a paste blocked
/// by an elevated target (UIPI) is not reported by Windows at all and stays silent.
fn send_ctrl_v() -> Result<(), PasteError> {
    let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS::default()
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let input = [
        key(VK_CONTROL, false),
        key(VIRTUAL_KEY(b'V' as u16), false),
        key(VIRTUAL_KEY(b'V' as u16), true),
        key(VK_CONTROL, true),
    ];
    // SAFETY: a valid array of INPUT structs with its correct size.
    let sent = unsafe { SendInput(&input, size_of::<INPUT>() as i32) };
    if sent != input.len() as u32 {
        // Read while the thread's last error still belongs to the failed call; an elevated
        // target (UIPI) reports none and still counts all four, so this is the blocked case.
        return Err(PasteError::SendInput {
            sent,
            source: crate::last_error(),
        });
    }
    Ok(())
}

/// Wipes what the paste left and puts the saved formats back. With a `sequence`, the restore is
/// skipped when the clipboard changed since that number was read: a foreign write that landed in
/// the retries before or during the open must not be clobbered. `None` means an unguarded undo
/// of our own half-done replacement.
fn restore_formats(saved: SavedFormats, sequence: Option<u32>) -> Result<(), PasteError> {
    let _clipboard = open_clipboard()?;
    if let Some(sequence) = sequence {
        // SAFETY: no preconditions; this only reads the session-wide counter.
        let current = unsafe { GetClipboardSequenceNumber() };
        if current != sequence {
            log::debug!(
                "the clipboard changed during the paste ({sequence} -> {current}); leaving it alone"
            );
            return Ok(());
        }
    }
    // SAFETY: the clipboard is open on this thread, and the guard above closes it again.
    unsafe { EmptyClipboard() }.map_err(PasteError::Clear)?;
    for (format, bytes) in saved {
        if let Err(e) = put_format(format, &bytes) {
            log::warn!("could not restore the clipboard format {format}: {e}");
        }
    }
    Ok(())
}

/// The text as NUL-terminated UTF-16, with `\n` turned into `\r\n`, which most Windows editors
/// want. An existing `\r\n` pair is left alone.
fn to_utf16_with_crlf(text: &str) -> Vec<u16> {
    let mut units = Vec::with_capacity(text.len() + 1);
    let mut previous_was_cr = false;
    for unit in text.encode_utf16() {
        match unit {
            0x0A if !previous_was_cr => units.extend_from_slice(&[0x0D, 0x0A]),
            unit => units.push(unit),
        }
        previous_was_cr = unit == 0x0D;
    }
    units.push(0);
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_utf16_with_a_terminator() {
        assert_eq!(to_utf16_with_crlf("aé"), [0x61, 0xE9, 0]);
    }

    #[test]
    fn turns_newlines_into_crlf() {
        assert_eq!(to_utf16_with_crlf("a\nb"), [0x61, 0x0D, 0x0A, 0x62, 0]);
    }

    #[test]
    fn leaves_existing_crlf_alone() {
        assert_eq!(to_utf16_with_crlf("a\r\nb"), [0x61, 0x0D, 0x0A, 0x62, 0]);
        assert_eq!(to_utf16_with_crlf("a\r"), [0x61, 0x0D, 0]);
    }
}
