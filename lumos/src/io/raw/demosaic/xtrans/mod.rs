//! X-Trans CFA demosaicing module.
//!
//! Provides demosaicing for Fujifilm X-Trans sensors which use a 6x6 CFA pattern
//! instead of the standard 2x2 Bayer pattern.
//!
//! The X-Trans pattern has ~55% green, ~22.5% red, and ~22.5% blue pixels arranged
//! so that every row and column contains all three colors.
//!
//! Uses the Markesteijn 1-pass algorithm: directional interpolation in 4 directions
//! with homogeneity-based selection for high-quality output.

mod hex_lookup;
pub(crate) mod markesteijn;
mod markesteijn_steps;

use std::time::Instant;

use common::CancelToken;

use crate::io::raw::BlackRepeat;
use crate::io::raw::demosaic::DemosaicError;
use crate::io::raw::demosaic::sensor_layout::SensorLayout;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// The u16 path's on-the-fly normalization: per-channel black levels, the
/// reciprocal of the range above black, and the repeating black pattern applied
/// before either. The f32 path arrives calibrated and takes none of it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct XTransNormalization<'a> {
    pub(crate) channel_black: [f32; 3],
    pub(crate) inv_range: f32,
    pub(crate) black_repeat: Option<&'a BlackRepeat>,
}

/// Process X-Trans sensor data and demosaic to RGB.
///
/// Takes raw u16 sensor data and normalization parameters. Normalization happens
/// on-the-fly during demosaicing, avoiding a separate P×4 byte f32 buffer.
///
/// Returns planar `[R, G, B]` channels, each `width * height`.
pub(crate) fn process_xtrans(
    raw_data: &[u16],
    layout: SensorLayout,
    raw_pattern: [[u8; 6]; 6],
    normalization: XTransNormalization<'_>,
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], DemosaicError> {
    let raw_pattern = XTransPattern::new(raw_pattern)?;
    let active = layout.active;

    let xtrans = XTransImage::with_margins(raw_data, layout, raw_pattern, normalization);

    let demosaic_start = Instant::now();
    let rgb_pixels = markesteijn::demosaic(&xtrans, cancel)?;
    let demosaic_elapsed = demosaic_start.elapsed();

    tracing::info!(
        "X-Trans Markesteijn demosaicing {}x{} took {:.2}ms",
        active.width,
        active.height,
        demosaic_elapsed.as_secs_f64() * 1000.0
    );

    Ok(rgb_pixels)
}

/// Process calibrated f32 X-Trans data and demosaic to RGB.
///
/// Avoids the lossy f32->u16->f32 roundtrip of converting to u16 for `process_xtrans`.
pub(crate) fn process_xtrans_f32(
    data: &[f32],
    layout: SensorLayout,
    raw_pattern: [[u8; 6]; 6],
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], DemosaicError> {
    let raw_pattern = XTransPattern::new(raw_pattern)?;
    let active = layout.active;

    let xtrans = XTransImage::with_margins_f32(data, layout, raw_pattern);

    let demosaic_start = Instant::now();
    let rgb_pixels = markesteijn::demosaic(&xtrans, cancel)?;
    let demosaic_elapsed = demosaic_start.elapsed();

    tracing::info!(
        "X-Trans Markesteijn demosaicing (f32) {}x{} took {:.2}ms",
        active.width,
        active.height,
        demosaic_elapsed.as_secs_f64() * 1000.0
    );

    Ok(rgb_pixels)
}

/// X-Trans 6x6 color filter array pattern.
/// Values: 0=Red, 1=Green, 2=Blue
#[derive(Debug, Clone)]
pub(crate) struct XTransPattern {
    /// 6x6 pattern array indexed by [row % 6][col % 6]
    pub(crate) pattern: [[u8; 6]; 6],
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum XTransPatternError {
    #[error(
        "invalid X-Trans pattern value {value} at row {row}, column {column}; expected 0, 1, or 2"
    )]
    Value {
        row: usize,
        column: usize,
        value: u8,
    },
    #[error("invalid X-Trans color counts: expected [8, 20, 8], got {actual:?}")]
    ColorCounts { actual: [usize; 3] },
    #[error("invalid X-Trans green neighborhood at row {row}, column {column}: {neighbors:?}")]
    GreenNeighborhood {
        row: usize,
        column: usize,
        neighbors: [usize; 3],
    },
}

impl XTransPattern {
    /// Create a new X-Trans pattern from a 6x6 array.
    pub(crate) fn new(pattern: [[u8; 6]; 6]) -> Result<Self, XTransPatternError> {
        let mut counts = [0usize; 3];
        for (row, values) in pattern.iter().enumerate() {
            for (column, &value) in values.iter().enumerate() {
                if value > 2 {
                    return Err(XTransPatternError::Value { row, column, value });
                }
                counts[value as usize] += 1;
            }
        }
        if counts != [8, 20, 8] {
            return Err(XTransPatternError::ColorCounts { actual: counts });
        }
        for row in 0..6 {
            for column in 0..6 {
                if pattern[row][column] != 1 {
                    continue;
                }
                let mut neighbors = [0usize; 3];
                for (dy, dx) in [(0, 1), (1, 0), (0, -1), (-1, 0)] {
                    let y = (row as i32 + dy).rem_euclid(6) as usize;
                    let x = (column as i32 + dx).rem_euclid(6) as usize;
                    neighbors[pattern[y][x] as usize] += 1;
                }
                if neighbors[0] != neighbors[2] {
                    return Err(XTransPatternError::GreenNeighborhood {
                        row,
                        column,
                        neighbors,
                    });
                }
            }
        }
        Ok(Self { pattern })
    }

    /// Get the color at position (row, col).
    /// Returns: 0=Red, 1=Green, 2=Blue
    #[inline(always)]
    pub(crate) fn color_at(&self, pos: Vec2us) -> u8 {
        self.pattern[pos.y % 6][pos.x % 6]
    }
}

/// Pixel data source: either raw u16 sensor values or calibrated f32.
///
/// The u16 path is used by the raw loader (saves ~47 MB by deferring normalization).
/// The f32 path is used by CfaImage after calibration (avoids lossy f32->u16 roundtrip).
#[derive(Debug)]
enum PixelSource<'a> {
    U16(&'a [u16]),
    U16WithRepeat {
        data: &'a [u16],
        repeat: &'a BlackRepeat,
    },
    F32(&'a [f32]),
}

/// Raw X-Trans image data with metadata needed for demosaicing.
///
/// Supports both raw u16 sensor data (with on-the-fly normalization) and
/// calibrated f32 data (identity passthrough).
#[derive(Debug)]
pub(crate) struct XTransImage<'a> {
    /// Pixel data (u16 raw sensor values or calibrated f32)
    data: PixelSource<'a>,
    /// Extent of the raw data buffer.
    pub(crate) raw: Size2us,
    /// Extent of the active/output image area.
    pub(crate) active: Size2us,
    /// Top-left corner of the active area within the raw buffer.
    pub(crate) margin: Vec2us,
    /// CFA pattern anchored at the full raw buffer origin.
    pub(crate) raw_pattern: XTransPattern,
    /// Per-channel black levels [R=0, G=1, B=2] for u16 path normalization.
    channel_black: [f32; 3],
    /// 1.0 / (maximum - common_black) for normalization (u16 path only).
    inv_range: f32,
}

impl<'a> XTransImage<'a> {
    /// Create from raw u16 sensor data with on-the-fly per-channel normalization.
    pub(crate) fn with_margins(
        data: &'a [u16],
        layout: SensorLayout,
        raw_pattern: XTransPattern,
        normalization: XTransNormalization<'a>,
    ) -> Self {
        layout.validate(data.len());
        let SensorLayout {
            raw,
            active,
            margin,
        } = layout;
        let XTransNormalization {
            channel_black,
            inv_range,
            black_repeat,
        } = normalization;
        let data = black_repeat.map_or(PixelSource::U16(data), |repeat| {
            PixelSource::U16WithRepeat { data, repeat }
        });
        Self {
            data,
            raw,
            active,
            margin,
            raw_pattern,
            channel_black,
            inv_range,
        }
    }

    /// Create from calibrated f32 data, including negative and above-unity samples.
    ///
    /// Used by CfaImage after calibration to avoid lossy f32->u16->f32 roundtrip.
    pub(crate) fn with_margins_f32(
        data: &'a [f32],
        layout: SensorLayout,
        raw_pattern: XTransPattern,
    ) -> Self {
        layout.validate(data.len());
        let SensorLayout {
            raw,
            active,
            margin,
        } = layout;
        Self {
            data: PixelSource::F32(data),
            raw,
            active,
            margin,
            raw_pattern,
            channel_black: [0.0; 3],
            inv_range: 1.0,
        }
    }

    /// Read a pixel and return its normalized raw-linear value.
    ///
    /// For u16 data: per-channel black subtraction and normalization.
    /// For f32 data: returns the calibrated value directly.
    #[inline(always)]
    pub(crate) fn read_normalized(&self, raw_y: usize, raw_x: usize) -> f32 {
        let idx = raw_y * self.raw.width + raw_x;
        match &self.data {
            PixelSource::U16(data) => {
                let val = data[idx] as f32;
                let ch = self.raw_pattern.color_at(Vec2us::new(raw_x, raw_y)) as usize;
                ((val - self.channel_black[ch]).max(0.0) * self.inv_range).min(1.0)
            }
            PixelSource::U16WithRepeat { data, repeat } => {
                let val = data[idx] as f32;
                let ch = self.raw_pattern.color_at(Vec2us::new(raw_x, raw_y)) as usize;
                let repeat_delta = repeat.at_raw(raw_y, raw_x, self.margin);
                ((val - self.channel_black[ch]) * self.inv_range - repeat_delta).clamp(0.0, 1.0)
            }
            PixelSource::F32(data) => data[idx],
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::io::raw::demosaic::sensor_layout::SensorLayout;
    use crate::io::raw::demosaic::xtrans::{XTransImage, XTransNormalization, XTransPattern};

    const TEST_PATTERN: [[u8; 6]; 6] = [
        [1, 1, 0, 1, 1, 2],
        [1, 1, 2, 1, 1, 0],
        [2, 0, 1, 0, 2, 1],
        [1, 1, 2, 1, 1, 0],
        [1, 1, 0, 1, 1, 2],
        [0, 2, 1, 2, 0, 1],
    ];

    pub(crate) const TEST_INV_RANGE: f32 = 1.0 / 65535.0;

    pub(crate) fn test_pattern_array() -> [[u8; 6]; 6] {
        TEST_PATTERN
    }

    pub(crate) fn test_pattern() -> XTransPattern {
        XTransPattern::new(TEST_PATTERN).unwrap()
    }

    pub(crate) fn to_u16(value: f32) -> u16 {
        (value * 65535.0).round() as u16
    }

    pub(crate) fn make_xtrans(data: &[u16], layout: SensorLayout) -> XTransImage<'_> {
        XTransImage::with_margins(
            data,
            layout,
            test_pattern(),
            XTransNormalization {
                channel_black: [0.0; 3],
                inv_range: TEST_INV_RANGE,
                black_repeat: None,
            },
        )
    }
}

#[cfg(test)]
mod tests;
