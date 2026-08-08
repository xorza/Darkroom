use crate::io::raw::demosaic::bayer::CfaPattern;

/// Sensor type detected from libraw metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SensorType {
    /// Monochrome sensor (no CFA)
    Monochrome,
    /// Standard 2x2 Bayer pattern (RGGB, BGGR, GRBG, GBRG)
    Bayer(CfaPattern),
    /// Fujifilm X-Trans 6x6 CFA pattern
    XTrans,
    /// Unknown CFA pattern (exotic sensors) - requires libraw fallback
    Unknown,
}

/// Detect sensor type from libraw filters and colors fields.
///
/// Returns:
/// - `SensorType::Monochrome` for monochrome sensors (no CFA)
/// - `SensorType::Bayer(pattern)` for known 2x2 Bayer patterns
/// - `SensorType::XTrans` for Fujifilm X-Trans sensors (filters=9)
/// - `SensorType::Unknown` for other exotic sensors
pub(crate) fn detect_sensor_type(filters: u32, colors: i32) -> SensorType {
    // Monochrome: no CFA pattern or single color channel
    if filters == 0 || colors == 1 {
        return SensorType::Monochrome;
    }

    // X-Trans: libraw uses filters=9 to indicate 6x6 X-Trans pattern
    if filters == 9 {
        return SensorType::XTrans;
    }

    // Try to match known Bayer patterns
    if let Some(pattern) = CfaPattern::from_filters(filters) {
        return SensorType::Bayer(pattern);
    }

    // Unknown pattern (other exotic sensors)
    SensorType::Unknown
}

#[cfg(test)]
mod tests {
    use crate::io::image::sensor::*;

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
    fn test_detect_sensor_type_xtrans() {
        // X-Trans (filters=9)
        assert_eq!(detect_sensor_type(9, 3), SensorType::XTrans);
    }

    #[test]
    fn test_detect_sensor_type_unknown() {
        // Other exotic patterns
        assert_eq!(detect_sensor_type(0x12345678, 3), SensorType::Unknown);
    }
}
