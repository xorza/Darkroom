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
    assert_eq!(fmt_bytes(0).to_string(), "0 B");
    assert_eq!(fmt_bytes(512).to_string(), "512 B");
    assert_eq!(fmt_bytes(1024).to_string(), "1.0 KB");
    assert_eq!(fmt_bytes(1536).to_string(), "1.5 KB"); // 1536 / 1024 = 1.5
    assert_eq!(fmt_bytes(1_048_576).to_string(), "1.0 MB"); // 1024^2
    assert_eq!(fmt_bytes(3_145_728).to_string(), "3.0 MB"); // 3 * 1024^2
    assert_eq!(fmt_bytes(1_073_741_824).to_string(), "1.00 GB"); // 1024^3
    assert_eq!(fmt_bytes(1_610_612_736).to_string(), "1.50 GB"); // 1.5 * 1024^3
}
