use super::demosaic::CfaPattern;

/// Sensor type detected from libraw metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorType {
    /// Monochrome sensor (no CFA)
    Monochrome,
    /// Standard 2x2 Bayer pattern (RGGB, BGGR, GRBG, GBRG)
    Bayer(CfaPattern),
    /// Unknown CFA pattern (X-Trans, exotic sensors) - requires libraw fallback
    Unknown,
}

/// Detect sensor type from libraw filters and colors fields.
///
/// Returns:
/// - `SensorType::Monochrome` for monochrome sensors (no CFA)
/// - `SensorType::Bayer(pattern)` for known 2x2 Bayer patterns
/// - `SensorType::Unknown` for X-Trans (filters=9) and other exotic sensors
pub fn detect_sensor_type(filters: u32, colors: i32) -> SensorType {
    // Monochrome: no CFA pattern or single color channel
    if filters == 0 || colors == 1 {
        return SensorType::Monochrome;
    }

    // Try to match known Bayer patterns
    if let Some(pattern) = cfa_pattern_from_filters(filters) {
        return SensorType::Bayer(pattern);
    }

    // Unknown pattern (X-Trans filters=9, or other exotic sensors)
    SensorType::Unknown
}

/// Extract CFA pattern from libraw filters field.
///
/// The filters field encodes the color at each position in a repeating pattern.
/// For Bayer sensors, this is a 2x2 pattern. The formula to extract color index is:
/// `color_index = (filters >> (((row << 1 & 14) | (col & 1)) << 1)) & 3`
///
/// Color indices: 0=Red, 1=Green, 2=Blue, 3=Green2
///
/// Returns None if the pattern doesn't match a known Bayer CFA pattern
/// (e.g., for X-Trans sensors or monochrome cameras).
pub fn cfa_pattern_from_filters(filters: u32) -> Option<CfaPattern> {
    // Extract color index for each position in the 2x2 pattern
    let color_at =
        |row: u32, col: u32| -> u32 { (filters >> (((row << 1 & 14) | (col & 1)) << 1)) & 3 };

    let c00 = color_at(0, 0);
    let c01 = color_at(0, 1);
    let c10 = color_at(1, 0);
    let c11 = color_at(1, 1);

    // Color indices: 0=R, 1=G, 2=B, 3=G2 (second green)
    // We treat both green indices (1 and 3) as green
    let is_red = |c: u32| c == 0;
    let is_green = |c: u32| c == 1 || c == 3;
    let is_blue = |c: u32| c == 2;

    // Match against known Bayer patterns
    // RGGB: R at (0,0), G at (0,1) and (1,0), B at (1,1)
    if is_red(c00) && is_green(c01) && is_green(c10) && is_blue(c11) {
        return Some(CfaPattern::Rggb);
    }
    // BGGR: B at (0,0), G at (0,1) and (1,0), R at (1,1)
    if is_blue(c00) && is_green(c01) && is_green(c10) && is_red(c11) {
        return Some(CfaPattern::Bggr);
    }
    // GRBG: G at (0,0), R at (0,1), B at (1,0), G at (1,1)
    if is_green(c00) && is_red(c01) && is_blue(c10) && is_green(c11) {
        return Some(CfaPattern::Grbg);
    }
    // GBRG: G at (0,0), B at (0,1), R at (1,0), G at (1,1)
    if is_green(c00) && is_blue(c01) && is_red(c10) && is_green(c11) {
        return Some(CfaPattern::Gbrg);
    }

    // Unknown pattern (e.g., X-Trans, monochrome, or other exotic sensors)
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cfa_pattern_from_filters_rggb() {
        // RGGB pattern: R=0, G=1, G=3, B=2
        // Position (0,0)=R=0, (0,1)=G=1, (1,0)=G=3, (1,1)=B=2
        // Encoded in filters with 2 bits per position
        // Common RGGB filters value from Canon cameras
        let filters = 0x94949494u32; // Standard RGGB encoding
        assert_eq!(cfa_pattern_from_filters(filters), Some(CfaPattern::Rggb));
    }

    #[test]
    fn test_cfa_pattern_from_filters_bggr() {
        // BGGR pattern: B at (0,0), G at (0,1), G at (1,0), R at (1,1)
        let filters = 0x16161616u32; // Standard BGGR encoding
        assert_eq!(cfa_pattern_from_filters(filters), Some(CfaPattern::Bggr));
    }

    #[test]
    fn test_cfa_pattern_from_filters_grbg() {
        // GRBG pattern: G at (0,0), R at (0,1), B at (1,0), G at (1,1)
        let filters = 0x61616161u32; // Standard GRBG encoding
        assert_eq!(cfa_pattern_from_filters(filters), Some(CfaPattern::Grbg));
    }

    #[test]
    fn test_cfa_pattern_from_filters_gbrg() {
        // GBRG pattern: G at (0,0), B at (0,1), R at (1,0), G at (1,1)
        let filters = 0x49494949u32; // Standard GBRG encoding
        assert_eq!(cfa_pattern_from_filters(filters), Some(CfaPattern::Gbrg));
    }

    #[test]
    fn test_cfa_pattern_from_filters_unknown() {
        // filters=0 typically indicates monochrome or no CFA
        assert_eq!(cfa_pattern_from_filters(0), None);
        // X-Trans and other exotic patterns should return None
        assert_eq!(cfa_pattern_from_filters(0x12345678), None);
    }

    #[test]
    fn test_detect_sensor_type_monochrome() {
        // filters == 0 indicates monochrome
        assert_eq!(detect_sensor_type(0, 3), SensorType::Monochrome);
        // colors == 1 also indicates monochrome
        assert_eq!(detect_sensor_type(0x94949494, 1), SensorType::Monochrome);
    }

    #[test]
    fn test_detect_sensor_type_bayer() {
        assert_eq!(
            detect_sensor_type(0x94949494, 3),
            SensorType::Bayer(CfaPattern::Rggb)
        );
        assert_eq!(
            detect_sensor_type(0x16161616, 3),
            SensorType::Bayer(CfaPattern::Bggr)
        );
    }

    #[test]
    fn test_detect_sensor_type_unknown() {
        // X-Trans (filters=9)
        assert_eq!(detect_sensor_type(9, 3), SensorType::Unknown);
        // Other exotic patterns
        assert_eq!(detect_sensor_type(0x12345678, 3), SensorType::Unknown);
    }
}
