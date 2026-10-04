//! [`BlackLevel`]: a RAW file's black level in ADU, folded as LibRaw folds it, without its
//! roundings.

use rayon::prelude::*;

use crate::io::raw::error::BlackLevelError;
use crate::io::raw::libraw_filter_color;
use crate::io::raw::sensor_layout::SensorLayout;
use crate::math::size2us::Size2us;

/// The length of LibRaw's `cblack` table: four channel values, the spatial pattern's height and
/// width, and the pattern.
const CBLACK_LEN: usize = 4104;
/// Where the spatial pattern starts in `cblack`.
const PATTERN: usize = 6;

/// What LibRaw reports of a file's black level once it is unpacked.
#[derive(Debug)]
pub(crate) struct LibrawBlack<'a> {
    pub(crate) black: u32,
    pub(crate) cblack: &'a [u32; CBLACK_LEN],
    pub(crate) maximum: u32,
    /// The visible area's `filters` word.
    pub(crate) filters: u32,
    /// A DNG's black levels as its tags state them, before LibRaw rounds them into `black` and
    /// `cblack`.
    pub(crate) dng: Option<DngLevels<'a>>,
    /// LibRaw's `black_stat`: the sums of the masked pixels per colour, then their counts.
    pub(crate) masked: [u32; 8],
}

/// A DNG's `dng_fblack` and `dng_fcblack`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DngLevels<'a> {
    pub(crate) black: f32,
    pub(crate) cblack: &'a [f32; CBLACK_LEN],
}

/// A file's black level in ADU: the common level, what each LibRaw colour channel adds, and a
/// spatial pattern on top, as LibRaw's `adjust_bl` leaves them.
///
/// LibRaw stores every term as a whole ADU, truncating or rounding what the file holds: a mean of
/// the masked pixels, or a DNG's rational `BlackLevel`. Where it keeps the unrounded value — the
/// masked sums in `black_stat`, the DNG levels in `dng_levels` — and that value rounds to LibRaw's
/// own, the unrounded one is used; elsewhere every term is LibRaw's integer, and exact.
#[derive(Debug)]
pub(crate) struct BlackLevel {
    common: f64,
    channel: [f64; 4],
    repeat: Option<BlackRepeat>,
    maximum: f64,
}

/// [`BlackLevel`]'s spatial pattern, anchored at the visible area's first pixel.
#[derive(Debug)]
struct BlackRepeat {
    size: Size2us,
    values: Box<[f64]>,
}

impl BlackLevel {
    /// The black level `libraw` reports, folded as `adjust_bl` folds it (LibRaw
    /// `utils_libraw.cpp`): a 2×2 or 1×1 spatial pattern into the channels, the channels' common
    /// part into the common level, and the spatial pattern's common part likewise.
    ///
    /// Every value is the file's, so each is checked: a pattern too large for LibRaw's table, and
    /// a black at or above `maximum` anywhere, are refused rather than normalized into nonsense.
    pub(crate) fn from_libraw(libraw: &LibrawBlack<'_>) -> Result<Self, BlackLevelError> {
        let raw = libraw.cblack;
        let (height, width) = (raw[4], raw[5]);
        let pattern_len = if height > 0 && width > 0 {
            let len = (height as usize)
                .checked_mul(width as usize)
                .ok_or(BlackLevelError::SpatialPatternOverflow { width, height })?;
            if len > CBLACK_LEN - PATTERN {
                return Err(BlackLevelError::SpatialPatternTooLarge {
                    width,
                    height,
                    capacity: CBLACK_LEN - PATTERN,
                });
            }
            len
        } else {
            0
        };
        let used = PATTERN + pattern_len;
        let mut black = f64::from(libraw.black);
        let mut cblack: Vec<f64> = raw[..used].iter().map(|&value| f64::from(value)).collect();

        // The unrounded terms, where LibRaw kept them and they are what it rounded.
        if let Some(dng) = libraw.dng {
            let near = |integer: f64, exact: f32| (integer - f64::from(exact)).abs() < 1.0;
            let terms = (0..4).chain(PATTERN..used);
            if near(black, dng.black) && terms.clone().all(|i| near(cblack[i], dng.cblack[i])) {
                black = f64::from(dng.black);
                for i in terms {
                    cblack[i] = f64::from(dng.cblack[i]);
                }
            }
        } else {
            let [sums @ .., _, _, _, _] = libraw.masked;
            let counts = &libraw.masked[4..];
            let masked_mean = libraw.black == 0
                && counts.iter().all(|&count| count > 0)
                && (0..4).all(|c| raw[c] == sums[c] / counts[c]);
            if masked_mean {
                for c in 0..4 {
                    cblack[c] = f64::from(sums[c]) / f64::from(counts[c]);
                }
            }
        }

        let mut pattern = Size2us::new(width as usize, height as usize);
        // A 2×2 pattern on a Bayer sensor, or a 1×1 one on X-Trans, is a per-channel level.
        if libraw.filters > 1000 && height.div_ceil(2) == 1 && width.div_ceil(2) == 1 {
            let mut colors = [0usize; 4];
            let mut last_green = None;
            for (c, color) in colors.iter_mut().enumerate() {
                *color = libraw_filter_color(libraw.filters, c / 2, c % 2);
                if *color == 1 {
                    last_green = Some(c);
                }
            }
            if colors.iter().filter(|&&color| color == 1).count() > 1
                && let Some(green) = last_green
            {
                colors[green] = 3;
            }
            for c in 0..4 {
                let cell = (c / 2) % height as usize * width as usize + c % 2 % width as usize;
                cblack[colors[c]] += cblack[PATTERN + cell];
            }
            pattern = Size2us::new(0, 0);
        } else if libraw.filters <= 1000 && height == 1 && width == 1 {
            for c in 0..4 {
                cblack[c] += cblack[PATTERN];
            }
            pattern = Size2us::new(0, 0);
        }

        let common_channel = cblack[..4].iter().copied().fold(f64::INFINITY, f64::min);
        for value in &mut cblack[..4] {
            *value -= common_channel;
        }
        black += common_channel;

        let repeat = if pattern.pixel_count() > 0 {
            let cells = &mut cblack[PATTERN..PATTERN + pattern.pixel_count()];
            let common_cell = cells.iter().copied().fold(f64::INFINITY, f64::min);
            for value in cells.iter_mut() {
                *value -= common_cell;
            }
            black += common_cell;
            cells
                .iter()
                .any(|&value| value != 0.0)
                .then(|| BlackRepeat {
                    size: pattern,
                    values: cells.into(),
                })
        } else {
            None
        };

        let level = Self {
            common: black,
            channel: [cblack[0], cblack[1], cblack[2], cblack[3]],
            repeat,
            maximum: f64::from(libraw.maximum),
        };
        let highest = level.common
            + level.channel.iter().copied().fold(0.0, f64::max)
            + level.repeat.as_ref().map_or(0.0, |repeat| {
                repeat.values.iter().copied().fold(0.0, f64::max)
            });
        if highest >= level.maximum {
            return Err(BlackLevelError::BlackExceedsMaximum {
                black: highest,
                maximum: libraw.maximum,
            });
        }
        tracing::debug!(
            common = level.common,
            channel = ?level.channel,
            repeat = ?level.repeat.as_ref().map(|repeat| repeat.size),
            span = level.span(),
            "black levels"
        );
        Ok(level)
    }

    /// `maximum − black`, in ADU: what one normalized unit is worth for this file. It differs
    /// between frames — LibRaw reads `maximum` per camera and per ISO, and `black` per frame — so
    /// two frames convert by the ratio of their spans (`SampleDomain::conversion_to`).
    pub(crate) fn span(&self) -> f64 {
        self.maximum - self.common
    }

    /// The black level of LibRaw colour channel `channel`, without the spatial pattern.
    pub(crate) fn of_channel(&self, channel: usize) -> f64 {
        self.common + self.channel[channel]
    }

    /// The black level at visible pixel `(x, y)` of colour channel `channel`.
    #[inline]
    fn at(&self, channel: usize, x: usize, y: usize) -> f64 {
        self.of_channel(channel)
            + self.repeat.as_ref().map_or(0.0, |repeat| {
                repeat.values[(y % repeat.size.height) * repeat.size.width + x % repeat.size.width]
            })
    }

    /// The visible area of `raw` as `(v − black(x, y)) / span`, in one pass: the subtraction is
    /// exact in f64 for every black LibRaw or the file states, and the quotient rounds to f32 once,
    /// correctly — an f64 quotient rounds to f32 without a second error, as f64 carries more than
    /// twice f32's digits. `channel` names each pixel's LibRaw colour channel.
    pub(crate) fn normalize(
        &self,
        raw: &[u16],
        layout: SensorLayout,
        channel: impl Fn(usize, usize) -> usize + Sync,
    ) -> Vec<f32> {
        let span = self.span();
        let active = layout.active;
        let mut pixels = vec![0.0f32; active.pixel_count()];
        pixels
            .par_chunks_mut(active.width)
            .enumerate()
            .for_each(|(y, row)| {
                let start = (layout.margin.y + y) * layout.raw.width + layout.margin.x;
                for (x, (pixel, &value)) in row.iter_mut().zip(&raw[start..]).enumerate() {
                    *pixel = ((f64::from(value) - self.at(channel(x, y), x, y)) / span) as f32;
                }
            });
        pixels
    }
}

#[cfg(test)]
mod tests;
