use crate::gui::widgets::format;
use crate::gui::widgets::format::bytes::Bytes;
use crate::gui::widgets::format::elapsed::Elapsed;
use crate::gui::widgets::format::percent::Percent;

/// Each unit switch on both sides, and the carry that shows a value rounding
/// up to the next unit's 1 in that unit. The node header measures these
/// against the width its label reserves.
#[test]
fn elapsed_steps_through_units_within_the_reserved_width() {
    // Both sides of every unit switch, plus the digit-count steps within
    // seconds, which is where a live timer spends its time.
    let cases = [
        (0.0, "0ns"),
        (0.4e-9, "<1ns"),
        // A half rounds to even, so 0.5 ns prints as 0 and reads as under one.
        (0.5e-9, "<1ns"),
        (0.6e-9, "1ns"),
        (100e-9, "100ns"),
        (999.4e-9, "999ns"),
        (1e-6, "1µs"),
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
        (999.6e-9, "1µs"),
        (999.6e-6, "1.0ms"),
        (0.999_96, "1.00s"),
        (999.996, "1000.0s"),
        (9_999.96, "10000s"),
        (999_999.0, "999999s"),
    ];
    for (secs, expected) in cases {
        assert_eq!(
            Elapsed::new(secs).to_string(),
            expected,
            "Elapsed::new({secs})"
        );
    }
}

#[test]
fn bytes_steps_through_magnitudes() {
    // Sub-KiB stays exact in bytes; each threshold is a power of 1024.
    assert_eq!(Bytes::new(0).to_string(), "0 B");
    assert_eq!(Bytes::new(512).to_string(), "512 B");
    assert_eq!(Bytes::new(1024).to_string(), "1.0 KiB");
    assert_eq!(Bytes::new(1536).to_string(), "1.5 KiB"); // 1536 / 1024 = 1.5
    assert_eq!(Bytes::new(1_048_576).to_string(), "1.0 MiB"); // 1024^2
    assert_eq!(Bytes::new(3_145_728).to_string(), "3.0 MiB"); // 3 * 1024^2
    assert_eq!(Bytes::new(1_073_741_824).to_string(), "1.00 GiB"); // 1024^3
    assert_eq!(Bytes::new(1_610_612_736).to_string(), "1.50 GiB"); // 1.5 * 1024^3
    // One below each threshold rounds up to 1024.0 of the smaller unit, so it reads as 1 of the
    // larger: 1_048_575 / 1024 = 1023.999…, (1024^3 − 1) / 1024^2 = 1023.999….
    assert_eq!(Bytes::new(1_023).to_string(), "1023 B");
    assert_eq!(Bytes::new(1_048_575).to_string(), "1.0 MiB");
    assert_eq!(Bytes::new(1_073_741_823).to_string(), "1.00 GiB");
}

/// Whole from 10 %, rounding half to even as `{:.0}` does (62.5 → 62); under 10 %, two
/// significant digits, so a fit far below a view's range never reads as 0%.
#[test]
fn percent_reads_whole_then_two_significant_digits() {
    let cases = [
        (5.0, "500%"),
        (1.25, "125%"),
        (0.62, "62%"),
        (0.625, "62%"),
        (0.1, "10%"),
        (0.05, "5.0%"),
        (0.004, "0.40%"),
        (0.000_4, "0.040%"),
    ];
    for (factor, expected) in cases {
        assert_eq!(Percent::new(factor).to_string(), expected, "{factor}");
    }
}

/// A node's name stands as it is; a cleared one reads "Unnamed" on every surface.
#[test]
fn a_cleared_node_name_reads_unnamed() {
    assert_eq!(format::node_name("Blur"), "Blur");
    assert_eq!(format::node_name(""), "Unnamed");
}
