//! [`Elapsed`]: a run time, formatted on demand.

use std::fmt;
use std::fmt::{Display, Formatter};

use crate::gui::widgets::format;

/// A run time that renders on demand rather than into a `String`.
///
/// `Display` rather than an owned buffer because every caller is a
/// per-frame label: the node header's live timer runs once per node per
/// frame and the inspector's status line once per open panel, and both
/// feed the result straight into a formatter that never needed it to be
/// heap-allocated.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Elapsed(f64);

impl Elapsed {
    /// Compact run-time label: seconds → `s` / `ms` / `µs` / `ns` at the scale
    /// that keeps 2–3 significant digits, and `<1ns` for a time above 0 that
    /// would print as `0ns`, so a run that took time never reads as none. The
    /// elapsed-time sibling of [`Bytes`](crate::gui::widgets::format::bytes::Bytes),
    /// read by the node header's live timer and the inspector's status line.
    ///
    /// Stays within 7 characters up to `999999s` (decimals drop past `999.99s`),
    /// which is what `RUN_TIME_MIN_WIDTH` in `gui::pane::graph_ui::node_ui::header`
    /// reserves so a running node's label measures identically across
    /// digit-count changes (see the test).
    pub(crate) const fn new(secs: f64) -> Self {
        Self(secs)
    }
}

impl Display for Elapsed {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let secs = self.0;
        // Each unit is chosen by what it would print, not by the raw value: 0.99996 s is shown as
        // "1.00s", never as "1000.0ms".
        if secs > 0.0 && format::printed_integer_part(secs * 1e9, 0) == 0 {
            f.write_str("<1ns")
        } else if secs < 1e-6 && format::printed_integer_part(secs * 1e9, 0) < 1000 {
            write!(f, "{:.0}ns", secs * 1e9)
        } else if secs < 1e-3 && format::printed_integer_part(secs * 1e6, 0) < 1000 {
            write!(f, "{:.0}µs", secs * 1e6)
        } else if secs < 1.0 && format::printed_integer_part(secs * 1e3, 1) < 1000 {
            write!(f, "{:.1}ms", secs * 1e3)
        } else if format::printed_integer_part(secs, 2) < 1000 {
            write!(f, "{secs:.2}s")
        } else if format::printed_integer_part(secs, 1) < 10_000 {
            // Past 999.99 s a decimal goes, so the label keeps its 7 characters.
            write!(f, "{secs:.1}s")
        } else {
            write!(f, "{secs:.0}s")
        }
    }
}
