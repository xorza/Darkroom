use super::*;

/// Each unit switch on both sides, and the carry that shows a value rounding
/// up to the next unit's 1 in that unit. The node header measures these
/// against the width its label reserves.
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
        // The carry side of every switch: a value that rounds up to the next unit's 1 is shown in
        // that unit, never as "1000" of the smaller one.
        (999.6e-6, "1.0ms"),
        (0.999_96, "1.00s"),
        (999.996, "1000.0s"),
        (9_999.96, "10000s"),
        (999_999.0, "999999s"),
    ];
    for (secs, expected) in cases {
        assert_eq!(
            fmt_elapsed(secs).to_string(),
            expected,
            "fmt_elapsed({secs})"
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
    // One below each threshold rounds up to 1024.0 of the smaller unit, so it reads as 1 of the
    // larger: 1_048_575 / 1024 = 1023.999…, (1024^3 − 1) / 1024^2 = 1023.999….
    assert_eq!(fmt_bytes(1_023).to_string(), "1023 B");
    assert_eq!(fmt_bytes(1_048_575).to_string(), "1.0 MB");
    assert_eq!(fmt_bytes(1_073_741_823).to_string(), "1.00 GB");
}
