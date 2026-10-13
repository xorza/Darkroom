//! [`Bytes`]: a byte magnitude, formatted on demand.

use std::fmt;
use std::fmt::{Display, Formatter};

use crate::gui::widgets::format;

/// A byte magnitude that renders on demand rather than into a `String`.
///
/// `Display` for the same reason
/// [`Elapsed`](crate::gui::widgets::format::elapsed::Elapsed) is: every reader
/// is a per-frame readout — the status bar's twice a frame, a node's memory
/// footer once per pool per node — and each feeds the result into a formatter
/// that never needed it heap-allocated.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bytes(u64);

impl Bytes {
    /// Human-readable byte magnitude (1024-based) — the byte analogue of
    /// [`Elapsed`](crate::gui::widgets::format::elapsed::Elapsed): bare `B`
    /// under 1 KB, then `KB`/`MB`/`GB` carrying 1–2 decimals. Used by the
    /// window status bar and each node body's memory readout, so both render
    /// identical figures.
    pub(crate) const fn new(bytes: u64) -> Self {
        Self(bytes)
    }
}

impl Display for Bytes {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        const KB: f64 = 1024.0;
        const MB: f64 = KB * KB;
        const GB: f64 = KB * KB * KB;
        let bytes = self.0;
        let b = bytes as f64;
        // As for `Elapsed`: a magnitude that rounds up to 1024 of a unit is shown as 1 of the next.
        if bytes < 1024 {
            write!(f, "{bytes} B")
        } else if b < MB && format::printed_integer_part(b / KB, 1) < 1024 {
            write!(f, "{:.1} KB", b / KB)
        } else if b < GB && format::printed_integer_part(b / MB, 1) < 1024 {
            write!(f, "{:.1} MB", b / MB)
        } else {
            write!(f, "{:.2} GB", b / GB)
        }
    }
}
