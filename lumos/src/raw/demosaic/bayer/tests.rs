//! Tests for Bayer CFA types.

use super::{BayerImage, CfaPattern};

// ── CFA pattern tests ────────────────────────────────────────

#[test]
fn test_cfa_rggb_pattern() {
    let cfa = CfaPattern::Rggb;
    assert_eq!(cfa.color_at(0, 0), 0); // R
    assert_eq!(cfa.color_at(0, 1), 1); // G
    assert_eq!(cfa.color_at(0, 2), 0); // R
    assert_eq!(cfa.color_at(0, 3), 1); // G
    assert_eq!(cfa.color_at(1, 0), 1); // G
    assert_eq!(cfa.color_at(1, 1), 2); // B
    assert_eq!(cfa.color_at(1, 2), 1); // G
    assert_eq!(cfa.color_at(1, 3), 2); // B
}

#[test]
fn test_cfa_bggr_pattern() {
    let cfa = CfaPattern::Bggr;
    assert_eq!(cfa.color_at(0, 0), 2); // B
    assert_eq!(cfa.color_at(0, 1), 1); // G
    assert_eq!(cfa.color_at(1, 0), 1); // G
    assert_eq!(cfa.color_at(1, 1), 0); // R
}

#[test]
fn test_cfa_grbg_pattern() {
    let cfa = CfaPattern::Grbg;
    assert_eq!(cfa.color_at(0, 0), 1); // G
    assert_eq!(cfa.color_at(0, 1), 0); // R
    assert_eq!(cfa.color_at(1, 0), 2); // B
    assert_eq!(cfa.color_at(1, 1), 1); // G
}

#[test]
fn test_cfa_gbrg_pattern() {
    let cfa = CfaPattern::Gbrg;
    assert_eq!(cfa.color_at(0, 0), 1); // G
    assert_eq!(cfa.color_at(0, 1), 2); // B
    assert_eq!(cfa.color_at(1, 0), 0); // R
    assert_eq!(cfa.color_at(1, 1), 1); // G
}

#[test]
fn test_red_in_row() {
    assert!(CfaPattern::Rggb.red_in_row(0));
    assert!(!CfaPattern::Rggb.red_in_row(1));
    assert!(CfaPattern::Rggb.red_in_row(2));

    assert!(!CfaPattern::Bggr.red_in_row(0));
    assert!(CfaPattern::Bggr.red_in_row(1));
    assert!(!CfaPattern::Bggr.red_in_row(2));

    assert!(CfaPattern::Grbg.red_in_row(0));
    assert!(!CfaPattern::Grbg.red_in_row(1));

    assert!(!CfaPattern::Gbrg.red_in_row(0));
    assert!(CfaPattern::Gbrg.red_in_row(1));
}

#[test]
fn test_pattern_2x2() {
    assert_eq!(CfaPattern::Rggb.pattern_2x2(), [0, 1, 1, 2]);
    assert_eq!(CfaPattern::Bggr.pattern_2x2(), [2, 1, 1, 0]);
    assert_eq!(CfaPattern::Grbg.pattern_2x2(), [1, 0, 2, 1]);
    assert_eq!(CfaPattern::Gbrg.pattern_2x2(), [1, 2, 0, 1]);
}

// ── BayerImage validation tests ──────────────────────────────

#[test]
#[should_panic(expected = "Output dimensions must be non-zero")]
fn test_bayer_image_zero_width() {
    let data = vec![0.0f32; 4];
    BayerImage::with_margins(&data, 2, 2, 0, 2, 0, 0, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Output dimensions must be non-zero")]
fn test_bayer_image_zero_height() {
    let data = vec![0.0f32; 4];
    BayerImage::with_margins(&data, 2, 2, 2, 0, 0, 0, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Data length")]
fn test_bayer_image_wrong_data_length() {
    let data = vec![0.0f32; 3];
    BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Top margin")]
fn test_bayer_image_margin_exceeds_height() {
    let data = vec![0.0f32; 4];
    BayerImage::with_margins(&data, 2, 2, 2, 2, 1, 0, CfaPattern::Rggb);
}

#[test]
#[should_panic(expected = "Left margin")]
fn test_bayer_image_margin_exceeds_width() {
    let data = vec![0.0f32; 4];
    BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 1, CfaPattern::Rggb);
}

#[test]
fn test_bayer_image_valid() {
    let data = vec![0.0f32; 16];
    let bayer = BayerImage::with_margins(&data, 4, 4, 2, 2, 1, 1, CfaPattern::Rggb);
    assert_eq!(bayer.raw_width, 4);
    assert_eq!(bayer.raw_height, 4);
    assert_eq!(bayer.width, 2);
    assert_eq!(bayer.height, 2);
    assert_eq!(bayer.top_margin, 1);
    assert_eq!(bayer.left_margin, 1);
}
