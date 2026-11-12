//! [`Bytes`]: a byte magnitude, formatted on demand.

use std::fmt;
use std::fmt::{Display, Formatter};

use crate::gui::widgets::format;

/// A byte magnitude, 1024-based, that renders on demand rather than into a `String`: bare `B`
/// under 1 KiB, then `KiB`/`MiB` at one decimal and `GiB` at two. The unit names are IEC
/// 80000-13's, as `KB` names 1000: `AGENTS.md`, "Names", "Bytes are KiB, MiB and GiB". The status
/// bar and each node body's memory readout format through it, so both show identical figures.
///
/// `Display` for the same reason [`Elapsed`](crate::gui::widgets::format::elapsed::Elapsed) is:
/// every reader is a per-frame readout that feeds a formatter, which needs no heap allocation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bytes(u64);

impl Bytes {
    pub(crate) const fn new(bytes: u64) -> Self {
        Self(bytes)
    }
}

impl Display for Bytes {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        const KIB: f64 = 1024.0;
        const MIB: f64 = KIB * KIB;
        const GIB: f64 = KIB * KIB * KIB;
        let bytes = self.0;
        let b = bytes as f64;
        // As for `Elapsed`: a magnitude that rounds up to 1024 of a unit is shown as 1 of the next.
        if bytes < 1024 {
            write!(f, "{bytes} B")
        } else if b < MIB && format::printed_integer_part(b / KIB, 1) < 1024 {
            write!(f, "{:.1} KiB", b / KIB)
        } else if b < GIB && format::printed_integer_part(b / MIB, 1) < 1024 {
            write!(f, "{:.1} MiB", b / MIB)
        } else {
            write!(f, "{:.2} GiB", b / GIB)
        }
    }
}
