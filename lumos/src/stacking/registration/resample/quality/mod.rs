//! Geometric support and interpolation-confidence maps.
//!
//! The two maps are emitted together and agree pixel for pixel: `coverage == 0` exactly where
//! `confidence == 0`. Outside the source footprint both are zero. Inside it, coverage vanishes only
//! where some axis has no in-bounds tap magnitude, which zeroes that axis's signed sum and with it
//! the confidence numerator — and no partial-support subset of these kernels cancels that sum on its
//! own, since each one keeps a centre tap outweighing its negative lobes.
//!
//! `combine` depends on that rather than re-deriving it: it gates a sample on coverage alone and
//! multiplies the weight by confidence, so a covered pixel at zero confidence would enter the
//! statistics weightless. See `PixelCoverage`, and `validate_warp_quality`, which holds
//! caller-supplied planes to the same pairing.
//!
//! Every pixel whose tap window lies wholly inside the source — all but a `kernel_radius`-wide band
//! at the frame's edge — takes [`SeparableTaps::interior_quality`], which is where nearly all of the
//! grid is and nearly all of the time goes. The clipping arithmetic runs only in that band.

use std::sync::OnceLock;

use rayon::prelude::*;

use crate::math::size2us::Size2us;
use crate::stacking::registration::config::InterpolationMethod;
use crate::stacking::registration::resample::kernel::LANCZOS_LUT_RESOLUTION;
use crate::stacking::registration::resample::{kernel, row};
use crate::stacking::registration::transform::WarpTransform;
use glam::Vec2;
use imaginarium::Buffer2;

#[cfg(test)]
mod tests;

/// The widest separable kernel here: Lanczos4's 8 taps per axis. Every method's weights share this
/// array so the quality tail is written once rather than per kernel width.
const MAX_TAPS: usize = 8;

/// The two tap-weight sums a confidence is built from: the signed sum, and the sum of squares.
///
/// Their ratio is Kish's effective sample size — how many equally-weighted taps the kernel's actual
/// weights are worth — which is the inverse white-noise variance the interpolation implies.
#[derive(Debug, Clone, Copy, Default)]
struct AxisSums {
    signed: f32,
    square: f32,
}

impl AxisSums {
    /// The sums over every tap, for a window that needs no clipping.
    fn of(weights: &[f32]) -> Self {
        let mut sums = Self::default();
        for &weight in weights {
            sums.signed += weight;
            sums.square += weight * weight;
        }
        sums
    }
}

/// One axis's tap weights, split into what the whole kernel carries and what its in-bounds taps do.
///
/// Only wanted where a window straddles the source border; [`AxisSums::of`] covers the interior,
/// where the two halves are equal by definition.
#[derive(Debug, Clone, Copy, Default)]
struct AxisWeightStats {
    magnitude: f32,
    in_magnitude: f32,
    in_sums: AxisSums,
}

#[derive(Debug, Clone, Copy, Default)]
struct SampleQuality {
    coverage: f32,
    confidence: f32,
}

#[derive(Debug)]
pub(super) struct Maps {
    pub(super) coverage: Buffer2<f32>,
    pub(super) confidence: Buffer2<f32>,
}

/// Where a Lanczos kernel's taps fall at one position, before any weight has been read.
///
/// Separate from [`SeparableTaps`] because the interior path never needs the weights: it takes the
/// tap-weight sums from [`lanczos_interior_sums`], indexed by the fraction. Deciding interiority
/// from the window alone is what lets that path skip the `2a` LUT reads per axis entirely.
#[derive(Debug, Clone, Copy)]
struct LanczosWindow {
    pos: Vec2,
    start_x: i32,
    start_y: i32,
    taps: usize,
    fx: f32,
    fy: f32,
}

impl LanczosWindow {
    fn new(pos: Vec2, a: usize) -> Self {
        let x0 = pos.x.floor() as i32;
        let y0 = pos.y.floor() as i32;
        Self {
            pos,
            start_x: x0 - a as i32 + 1,
            start_y: y0 - a as i32 + 1,
            taps: 2 * a,
            fx: pos.x - x0 as f32,
            fy: pos.y - y0 as f32,
        }
    }

    fn is_interior(&self, size: Size2us) -> bool {
        let taps = self.taps as i32;
        self.start_x >= 0
            && self.start_y >= 0
            && self.start_x + taps <= size.width as i32
            && self.start_y + taps <= size.height as i32
    }
}

/// The `2a` Lanczos tap weights for fractional offset `f`, into the first `2a` slots of `weights`.
///
/// `row`'s distance convention, so the two cannot disagree about which coefficient a tap carries:
/// `(a-1-i) + f` below the centre and `(i+1-a) - f` above it, both non-negative, which is what lets
/// the lookup skip its sign handling.
fn lanczos_weights(a: usize, f: f32, weights: &mut [f32; MAX_TAPS]) {
    let lut = kernel::get_lanczos_lut(a);
    for (i, weight) in weights.iter_mut().take(2 * a).enumerate() {
        let distance = if i < a {
            (a - 1 - i) as f32 + f
        } else {
            (i + 1 - a) as f32 - f
        };
        *weight = lut.lookup_positive(distance);
    }
}

/// The tap-weight sums of an unclipped Lanczos kernel, for every fraction its LUT distinguishes.
///
/// An interior pixel's confidence depends on the fractional offset and nothing else, and the tap
/// weights are *already* a step function of it: each tap reads the LUT at `(offset + f)·RES + 0.5`
/// truncated, and the integer offset contributes exactly, so the whole window is decided by
/// `round(f·RES)`. There are therefore only `RES + 1` distinct sums per kernel width, and computing
/// them per pixel re-derives one of a few thousand values from `2a` table reads.
///
/// Reading them back is not quite free of error: `offset + f` is rounded to f32 before scaling, so
/// a fraction within an ulp of an index boundary can take its weights from the neighbouring entry.
/// That is bounded by one table step — measured at 6.5e-4 relative on the per-axis sums, and 7.7e-4
/// on the confidence they form once both axes are off the same way, against the ~2.4e-4 the kernel
/// weights are already quantized to. `tabulated_interior_sums_track_the_computed_ones` holds it
/// there. Confidence scales a sample's weight and is never compared against a threshold, so an
/// error three parts in ten thousand moves no decision.
fn lanczos_interior_sums(a: usize) -> &'static [AxisSums] {
    static SUMS: [OnceLock<Vec<AxisSums>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let slot = &SUMS[a - 2];
    slot.get_or_init(|| {
        (0..=LANCZOS_LUT_RESOLUTION)
            .map(|index| {
                let mut weights = [0.0; MAX_TAPS];
                lanczos_weights(
                    a,
                    index as f32 / LANCZOS_LUT_RESOLUTION as f32,
                    &mut weights,
                );
                AxisSums::of(&weights[..2 * a])
            })
            .collect()
    })
}

/// The entry of [`lanczos_interior_sums`] a fractional offset selects — the same rounding the tap
/// lookups apply, so the sums come from the weights the border path would have computed.
fn fraction_index(f: f32) -> usize {
    debug_assert!((0.0..=1.0).contains(&f));
    (f * LANCZOS_LUT_RESOLUTION as f32 + 0.5) as usize
}

/// What a kernel's confidence falls back to where its window straddles the source border.
///
/// Travels with the taps rather than being chosen at the call site: the rule belongs to the kernel
/// that produced them, and a window carrying the wrong one would report a confidence for
/// coefficients the sampler never used.
#[derive(Debug, Clone, Copy)]
enum BorderConfidence {
    /// The kernel's own surviving coefficients. Bilinear and bicubic stay well-conditioned when
    /// truncated, so what is left still describes what the sampler reads there.
    OwnCoefficients,
    /// Edge-extended bilinear at the clamped position. Truncating a signed Lanczos kernel can leave
    /// arbitrarily little weight, so [`row::lanczos`] samples that instead — and the confidence has
    /// to describe what was actually sampled, not the kernel that was abandoned.
    ClampedBilinear,
}

/// Where a separable kernel's taps land at one source position, and the weights they carry.
///
/// One shape for every method — `taps` says how many of the [`MAX_TAPS`] slots are in use — so the
/// floor/fract setup and the coverage/confidence tail below exist once instead of per kernel. Each
/// constructor also fixes the [`BorderConfidence`] its kernel answers to, which is what leaves
/// [`Self::quality`] the single entry point: there is no second method to reach for, and no way to
/// pair a kernel's taps with another's border rule.
#[derive(Debug)]
struct SeparableTaps {
    pos: Vec2,
    start_x: i32,
    start_y: i32,
    taps: usize,
    wx: [f32; MAX_TAPS],
    wy: [f32; MAX_TAPS],
    border: BorderConfidence,
}

impl SeparableTaps {
    /// The 2×2 window, whose weights are the fractional distances themselves.
    fn bilinear(pos: Vec2) -> Self {
        let x0 = pos.x.floor() as i32;
        let y0 = pos.y.floor() as i32;
        let fx = pos.x - x0 as f32;
        let fy = pos.y - y0 as f32;
        let mut taps = Self::empty(pos, x0, y0, 2, BorderConfidence::OwnCoefficients);
        taps.wx[..2].copy_from_slice(&[1.0 - fx, fx]);
        taps.wy[..2].copy_from_slice(&[1.0 - fy, fy]);
        taps
    }

    /// The 4×4 Catmull-Rom window, starting one pixel before the sample.
    fn bicubic(pos: Vec2) -> Self {
        let x0 = pos.x.floor() as i32;
        let y0 = pos.y.floor() as i32;
        let fx = pos.x - x0 as f32;
        let fy = pos.y - y0 as f32;
        let mut taps = Self::empty(pos, x0 - 1, y0 - 1, 4, BorderConfidence::OwnCoefficients);
        taps.wx[..4].copy_from_slice(&kernel::bicubic_weights(fx));
        taps.wy[..4].copy_from_slice(&kernel::bicubic_weights(fy));
        taps
    }

    /// The `2a`×`2a` Lanczos window, read from the same LUT the row warp samples through.
    fn lanczos(window: LanczosWindow, a: usize) -> Self {
        let mut taps = Self::empty(
            window.pos,
            window.start_x,
            window.start_y,
            2 * a,
            BorderConfidence::ClampedBilinear,
        );
        lanczos_weights(a, window.fx, &mut taps.wx);
        lanczos_weights(a, window.fy, &mut taps.wy);
        taps
    }

    fn empty(pos: Vec2, start_x: i32, start_y: i32, taps: usize, border: BorderConfidence) -> Self {
        debug_assert!(taps <= MAX_TAPS);
        Self {
            pos,
            start_x,
            start_y,
            taps,
            wx: [0.0; MAX_TAPS],
            wy: [0.0; MAX_TAPS],
            border,
        }
    }

    fn x_weights(&self) -> &[f32] {
        &self.wx[..self.taps]
    }

    fn y_weights(&self) -> &[f32] {
        &self.wy[..self.taps]
    }

    /// Whether every tap on both axes lands inside the source.
    fn is_interior(&self, size: Size2us) -> bool {
        let taps = self.taps as i32;
        self.start_x >= 0
            && self.start_y >= 0
            && self.start_x + taps <= size.width as i32
            && self.start_y + taps <= size.height as i32
    }

    /// Quality for a window with real data behind every tap.
    ///
    /// Coverage is exactly 1 — the in-bounds magnitudes *are* the whole kernel's — so the ratio and
    /// its clamp are skipped, and the confidence sums need no per-tap bounds test. This is the case
    /// for all but a border band, so it carries the pass.
    fn interior_quality(&self) -> SampleQuality {
        SampleQuality {
            coverage: 1.0,
            confidence: separable_confidence(
                AxisSums::of(self.x_weights()),
                AxisSums::of(self.y_weights()),
            ),
        }
    }

    fn clipped_x(&self, size: Size2us) -> AxisWeightStats {
        axis_weight_stats(self.start_x, self.x_weights(), size.width)
    }

    fn clipped_y(&self, size: Size2us) -> AxisWeightStats {
        axis_weight_stats(self.start_y, self.y_weights(), size.height)
    }

    /// Support and confidence at this position: the interior shortcut where the window is wholly
    /// inside the source, and this kernel's own [`BorderConfidence`] rule where it is not.
    fn quality(&self, size: Size2us) -> SampleQuality {
        if self.is_interior(size) {
            return self.interior_quality();
        }
        let x = self.clipped_x(size);
        let y = self.clipped_y(size);
        let coverage = separable_coverage(x, y);
        let confidence = match self.border {
            BorderConfidence::OwnCoefficients => separable_confidence(x.in_sums, y.in_sums),
            // Defensive, for the pairing the module doc describes: the clamped position is always
            // in bounds, so were every in-bounds tap to land on a kernel zero the fallback would
            // report a confident sample at zero coverage. Inside the footprint some tap normally
            // carries weight, so this does not otherwise fire.
            BorderConfidence::ClampedBilinear if coverage == 0.0 => 0.0,
            // `bilinear` answers to `OwnCoefficients`, so this recurs exactly once.
            BorderConfidence::ClampedBilinear => {
                Self::bilinear(kernel::clamp_to_pixel_centers(self.pos, size))
                    .quality(size)
                    .confidence
            }
        };
        SampleQuality {
            coverage,
            confidence,
        }
    }
}

fn axis_weight_stats(start: i32, weights: &[f32], length: usize) -> AxisWeightStats {
    let mut stats = AxisWeightStats::default();
    for (i, &weight) in weights.iter().enumerate() {
        let magnitude = weight.abs();
        stats.magnitude += magnitude;
        let coordinate = start + i as i32;
        if coordinate >= 0 && (coordinate as usize) < length {
            stats.in_sums.signed += weight;
            stats.in_magnitude += magnitude;
            stats.in_sums.square += weight * weight;
        }
    }
    stats
}

fn separable_coverage(x: AxisWeightStats, y: AxisWeightStats) -> f32 {
    let total = x.magnitude * y.magnitude;
    if total <= f32::EPSILON {
        0.0
    } else {
        ((x.in_magnitude * y.in_magnitude) / total).clamp(0.0, 1.0)
    }
}

fn separable_confidence(x: AxisSums, y: AxisSums) -> f32 {
    let normalization = x.signed * y.signed;
    let square = x.square * y.square;
    if normalization.abs() <= 1e-10 || square <= f32::EPSILON {
        0.0
    } else {
        normalization * normalization / square
    }
}

fn quality_at(pos: Vec2, size: Size2us, method: InterpolationMethod) -> SampleQuality {
    if !kernel::source_footprint_contains(pos, size) {
        return SampleQuality::default();
    }
    match method {
        InterpolationMethod::Nearest => SampleQuality {
            coverage: 1.0,
            confidence: 1.0,
        },
        InterpolationMethod::Bilinear => SeparableTaps::bilinear(pos).quality(size),
        InterpolationMethod::Bicubic => SeparableTaps::bicubic(pos).quality(size),
        InterpolationMethod::Lanczos2
        | InterpolationMethod::Lanczos3
        | InterpolationMethod::Lanczos4 => {
            let a = method.lanczos_param().unwrap();
            let window = LanczosWindow::new(pos, a);
            if window.is_interior(size) {
                // Every tap has data behind it, so coverage is exactly 1 and the sums are the
                // whole kernel's — which is a table lookup per axis rather than `2a` LUT reads
                // and their summation. This is all but a border band of the grid.
                let sums = lanczos_interior_sums(a);
                return SampleQuality {
                    coverage: 1.0,
                    confidence: separable_confidence(
                        sums[fraction_index(window.fx)],
                        sums[fraction_index(window.fy)],
                    ),
                };
            }
            SeparableTaps::lanczos(window, a).quality(size)
        }
    }
}

pub(super) fn maps(size: Size2us, transform: &WarpTransform, method: InterpolationMethod) -> Maps {
    let mut coverage = Buffer2::new_default(size.width, size.height);
    let mut confidence = Buffer2::new_default(size.width, size.height);
    coverage
        .pixels_mut()
        .par_chunks_mut(size.width)
        .zip(confidence.pixels_mut().par_chunks_mut(size.width))
        .enumerate()
        .for_each(|(y, (coverage_row, confidence_row))| {
            row::for_each_source_position(y, transform, size.width, |x, pos| {
                let quality =
                    pos.map_or_else(SampleQuality::default, |pos| quality_at(pos, size, method));
                coverage_row[x] = quality.coverage;
                confidence_row[x] = quality.confidence;
            });
        });

    Maps {
        coverage,
        confidence,
    }
}
