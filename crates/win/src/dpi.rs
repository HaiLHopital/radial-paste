//! Process-wide DPI awareness.
//!
//! The cursor, the monitors and the overlay all work in physical pixels. Without per-monitor
//! awareness, Windows virtualises those coordinates and the ring lands in the wrong place at
//! the wrong size. eframe asks for the same setting when the settings window opens, but the
//! overlay can appear before that, so the process asks for it first thing in `main`.

use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

/// Asks for Per-Monitor V2 awareness. Returns false if Windows refused, which only happens when
/// the awareness is already set - by an earlier call, or by the manifest planned for M5 - and
/// is then in effect anyway.
pub fn enable_per_monitor_v2() -> bool {
    // SAFETY: a process-wide setting that owns no resources; failing only means it was set
    // before.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.is_ok()
}
