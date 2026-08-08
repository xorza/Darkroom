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

impl SensorType {
    /// Classify a sensor from libraw's `filters` and `colors` metadata fields.
    ///
    /// Returns:
    /// - `Monochrome` for monochrome sensors (no CFA)
    /// - `Bayer(pattern)` for known 2x2 Bayer patterns
    /// - `XTrans` for Fujifilm X-Trans sensors (filters=9)
    /// - `Unknown` for other exotic sensors
    pub(crate) fn from_libraw(filters: u32, colors: i32) -> Self {
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
}

#[cfg(test)]
mod tests {
    use crate::io::image::sensor::*;

    #[test]
    fn test_from_libraw_monochrome() {
        // filters == 0 indicates monochrome
        assert_eq!(SensorType::from_libraw(0, 3), SensorType::Monochrome);
        // colors == 1 also indicates monochrome
        assert_eq!(
            SensorType::from_libraw(0x94949494, 1),
            SensorType::Monochrome
        );
    }

    #[test]
    fn test_from_libraw_bayer() {
        assert_eq!(
            SensorType::from_libraw(0x94949494, 3),
            SensorType::Bayer(CfaPattern::Rggb)
        );
        assert_eq!(
            SensorType::from_libraw(0x16161616, 3),
            SensorType::Bayer(CfaPattern::Bggr)
        );
    }

    #[test]
    fn test_from_libraw_xtrans() {
        // X-Trans (filters=9)
        assert_eq!(SensorType::from_libraw(9, 3), SensorType::XTrans);
    }

    #[test]
    fn test_from_libraw_unknown() {
        // Other exotic patterns
        assert_eq!(SensorType::from_libraw(0x12345678, 3), SensorType::Unknown);
    }
}
