//! Small display formatters shared across the GUI chrome.

/// Human-readable byte magnitude (1024-based) — the byte analogue of the node
/// header's `fmt_elapsed`: bare `B` under 1 KB, then `KB`/`MB`/`GB` carrying
/// 1–2 decimals. Used by the window status bar and each node body's memory
/// readout, so both render identical figures.
pub(crate) fn fmt_bytes(bytes: usize) -> String {
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
