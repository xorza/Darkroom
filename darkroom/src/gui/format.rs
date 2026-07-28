//! Small display formatters shared across the GUI chrome. Every reader of a
//! magnitude — the status bar, a node's memory footer and run-time label, the
//! inspector's status line, a preview card's info strip — formats through here, so
//! the same number never renders two ways.

/// Compact run-time label: seconds → `s` / `ms` / `µs` at the scale that keeps
/// 2–3 significant digits. The elapsed-time sibling of [`fmt_bytes`], read by
/// the node header's live timer and the inspector's status line.
///
/// Stays within 7 characters up to `999.99s`, which is what
/// `node::header::RUN_TIME_MIN_WIDTH` reserves so a running node's label
/// measures identically across digit-count changes (see the test).
pub(crate) fn fmt_elapsed(secs: f64) -> Elapsed {
    Elapsed(secs)
}

/// A run time that renders on demand rather than into a `String`.
///
/// `Display` rather than an owned buffer because every caller is a
/// per-frame label: the node header's live timer runs once per node per
/// frame and the inspector's status line once per open panel, and both
/// feed the result straight into a formatter that never needed it to be
/// heap-allocated.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Elapsed(f64);

impl std::fmt::Display for Elapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let secs = self.0;
        if secs >= 1.0 {
            write!(f, "{secs:.2}s")
        } else if secs >= 1e-3 {
            write!(f, "{:.1}ms", secs * 1e3)
        } else {
            write!(f, "{:.0}µs", secs * 1e6)
        }
    }
}

/// Human-readable byte magnitude (1024-based) — the byte analogue of
/// [`fmt_elapsed`]: bare `B` under 1 KB, then `KB`/`MB`/`GB` carrying
/// 1–2 decimals. Used by the window status bar and each node body's memory
/// readout, so both render identical figures.
pub(crate) fn fmt_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * KB;
    const GB: f64 = KB * KB * KB;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `node::header::RUN_TIME_MIN_WIDTH` reserves ~7 mono glyphs so a running
    /// node's label measures the same across every digit-count change, which
    /// is what stops its outgoing wires twitching mid-run. That only holds
    /// while `fmt_elapsed` stays inside 7 characters — widen it and the floor
    /// silently stops covering the common range.
    #[test]
    fn fmt_elapsed_steps_through_units_within_the_reserved_width() {
        // Both sides of every unit switch, plus the digit-count steps within
        // seconds, which is where a live timer spends its time.
        let cases = [
            (0.0, "0µs"),
            (9.99e-7, "1µs"),
            (999.4e-6, "999µs"),
            (1e-3, "1.0ms"),
            (999.94e-3, "999.9ms"),
            (1.0, "1.00s"),
            (9.994, "9.99s"),
            (10.0, "10.00s"),
            (99.999, "100.00s"),
            (999.994, "999.99s"),
        ];
        for (secs, expected) in cases {
            let got = fmt_elapsed(secs).to_string();
            assert_eq!(got, expected, "fmt_elapsed({secs})");
            assert!(
                got.chars().count() <= 7,
                "{got:?} is {} chars — past what RUN_TIME_MIN_WIDTH reserves",
                got.chars().count(),
            );
        }
    }

    #[test]
    fn fmt_bytes_steps_through_magnitudes() {
        // Sub-KB stays exact in bytes; each threshold is a power of 1024.
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1024), "1.0 KB");
        assert_eq!(fmt_bytes(1536), "1.5 KB"); // 1536 / 1024 = 1.5
        assert_eq!(fmt_bytes(1_048_576), "1.0 MB"); // 1024^2
        assert_eq!(fmt_bytes(3_145_728), "3.0 MB"); // 3 * 1024^2
        assert_eq!(fmt_bytes(1_073_741_824), "1.00 GB"); // 1024^3
        assert_eq!(fmt_bytes(1_610_612_736), "1.50 GB"); // 1.5 * 1024^3
    }
}
