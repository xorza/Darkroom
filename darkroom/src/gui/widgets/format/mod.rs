//! Small display formatters shared across the GUI chrome. Every reader of a
//! magnitude — the status bar, a node's memory footer and run-time label, the
//! inspector's status line, a preview card's info strip — formats through here, so
//! the same number never renders two ways.

/// Compact run-time label: seconds → `s` / `ms` / `µs` at the scale that keeps
/// 2–3 significant digits. The elapsed-time sibling of [`fmt_bytes`], read by
/// the node header's live timer and the inspector's status line.
///
/// Stays within 7 characters up to `999999s` (decimals drop past `999.99s`),
/// which is what `node::header::RUN_TIME_MIN_WIDTH` reserves so a running
/// node's label measures identically across digit-count changes (see the test).
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
        // Each unit is chosen by what it would print, not by the raw value: 0.99996 s is shown as
        // "1.00s", never as "1000.0ms".
        if secs < 1e-3 && printed_integer_part(secs * 1e6, 0) < 1000 {
            write!(f, "{:.0}µs", secs * 1e6)
        } else if secs < 1.0 && printed_integer_part(secs * 1e3, 1) < 1000 {
            write!(f, "{:.1}ms", secs * 1e3)
        } else if printed_integer_part(secs, 2) < 1000 {
            write!(f, "{secs:.2}s")
        } else if printed_integer_part(secs, 1) < 10_000 {
            // Past 999.99 s a decimal goes, so the label keeps its 7 characters.
            write!(f, "{secs:.1}s")
        } else {
            write!(f, "{secs:.0}s")
        }
    }
}

/// The integer part `value` prints with at `precision` decimals, read off the formatter itself so a
/// unit decision agrees with the digits the label then shows — including where they round up. A
/// stack buffer, because every caller formats once per frame.
fn printed_integer_part(value: f64, precision: usize) -> u64 {
    use std::io::Write as _;
    let mut buffer = [0u8; 48];
    let mut cursor = std::io::Cursor::new(&mut buffer[..]);
    write!(cursor, "{value:.precision$}").expect("48 bytes hold every magnitude formatted here");
    let len = usize::try_from(cursor.position()).expect("the cursor sits inside the buffer");
    let printed = &buffer[..len];
    let integer = printed
        .split(|&byte| byte == b'.')
        .next()
        .unwrap_or(printed);
    std::str::from_utf8(integer)
        .ok()
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(u64::MAX)
}

/// Human-readable byte magnitude (1024-based) — the byte analogue of
/// [`fmt_elapsed`]: bare `B` under 1 KB, then `KB`/`MB`/`GB` carrying
/// 1–2 decimals. Used by the window status bar and each node body's memory
/// readout, so both render identical figures.
pub(crate) fn fmt_bytes(bytes: u64) -> Bytes {
    Bytes(bytes)
}

/// A byte magnitude that renders on demand rather than into a `String`.
///
/// `Display` for the same reason [`Elapsed`] is: every reader is a per-frame
/// readout — the status bar's twice a frame, a node's memory footer once per
/// pool per node — and each feeds the result into a formatter that never
/// needed it heap-allocated.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bytes(u64);

impl std::fmt::Display for Bytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const KB: f64 = 1024.0;
        const MB: f64 = KB * KB;
        const GB: f64 = KB * KB * KB;
        let bytes = self.0;
        let b = bytes as f64;
        // As for `Elapsed`: a magnitude that rounds up to 1024 of a unit is shown as 1 of the next.
        if bytes < 1024 {
            write!(f, "{bytes} B")
        } else if b < MB && printed_integer_part(b / KB, 1) < 1024 {
            write!(f, "{:.1} KB", b / KB)
        } else if b < GB && printed_integer_part(b / MB, 1) < 1024 {
            write!(f, "{:.1} MB", b / MB)
        } else {
            write!(f, "{:.2} GB", b / GB)
        }
    }
}

#[cfg(test)]
mod tests;
