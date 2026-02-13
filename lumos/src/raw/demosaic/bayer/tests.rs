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

// ── from_bayerpat tests ──────────────────────────────────────

#[test]
fn test_from_bayerpat() {
    assert_eq!(CfaPattern::from_bayerpat("RGGB"), Some(CfaPattern::Rggb));
    assert_eq!(CfaPattern::from_bayerpat("BGGR"), Some(CfaPattern::Bggr));
    assert_eq!(CfaPattern::from_bayerpat("GRBG"), Some(CfaPattern::Grbg));
    assert_eq!(CfaPattern::from_bayerpat("GBRG"), Some(CfaPattern::Gbrg));
    // Case insensitive
    assert_eq!(CfaPattern::from_bayerpat("rggb"), Some(CfaPattern::Rggb));
    // "TRUE" is RGGB
    assert_eq!(CfaPattern::from_bayerpat("TRUE"), Some(CfaPattern::Rggb));
    // Whitespace trimmed
    assert_eq!(CfaPattern::from_bayerpat(" BGGR "), Some(CfaPattern::Bggr));
    // Invalid
    assert_eq!(CfaPattern::from_bayerpat("XXXX"), None);
    assert_eq!(CfaPattern::from_bayerpat(""), None);
}

// ── flip tests ───────────────────────────────────────────────

#[test]
fn test_flip_vertical() {
    // Flip swaps rows: RGGB row0=[R,G] row1=[G,B] → row0=[G,B] row1=[R,G] = GBRG
    assert_eq!(CfaPattern::Rggb.flip_vertical(), CfaPattern::Gbrg);
    assert_eq!(CfaPattern::Gbrg.flip_vertical(), CfaPattern::Rggb);
    assert_eq!(CfaPattern::Bggr.flip_vertical(), CfaPattern::Grbg);
    assert_eq!(CfaPattern::Grbg.flip_vertical(), CfaPattern::Bggr);
    // Double flip is identity
    assert_eq!(
        CfaPattern::Rggb.flip_vertical().flip_vertical(),
        CfaPattern::Rggb
    );
}

#[test]
fn test_flip_horizontal() {
    // Flip swaps columns: RGGB row0=[R,G] row1=[G,B] → row0=[G,R] row1=[B,G] = GRBG
    assert_eq!(CfaPattern::Rggb.flip_horizontal(), CfaPattern::Grbg);
    assert_eq!(CfaPattern::Grbg.flip_horizontal(), CfaPattern::Rggb);
    assert_eq!(CfaPattern::Bggr.flip_horizontal(), CfaPattern::Gbrg);
    assert_eq!(CfaPattern::Gbrg.flip_horizontal(), CfaPattern::Bggr);
    // Double flip is identity
    assert_eq!(
        CfaPattern::Rggb.flip_horizontal().flip_horizontal(),
        CfaPattern::Rggb
    );
}

#[test]
fn test_flip_both_axes() {
    // Flipping both axes is equivalent to 180° rotation
    // RGGB → flip_v → GBRG → flip_h → BGGR
    assert_eq!(
        CfaPattern::Rggb.flip_vertical().flip_horizontal(),
        CfaPattern::Bggr
    );
    assert_eq!(
        CfaPattern::Bggr.flip_vertical().flip_horizontal(),
        CfaPattern::Rggb
    );
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
