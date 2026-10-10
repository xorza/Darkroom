//! [`WarpKernel`]: one output pixel's tap weights, from the filter a warp names, stretched where
//! the warp shrinks the frame.

use glam::{DMat2, DVec2};

use crate::io::image::pixel_flags::Reach;
use crate::math::size2us::Size2us;
use crate::registration::registration_config::InterpolationMethod;
use crate::registration::resample::kernel::{LANCZOS_LUT_RESOLUTION, LanczosLut, LanczosOrder};
use crate::registration::resample::source_position::SourcePosition;
use crate::registration::transform::{TransformType, WarpTransform};
use crate::simd::{F32_LANES, F32x8, Isa, Mask8};

/// The spacing, in output pixels, of the grid a non-affine warp's local scale is read on.
///
/// A homography's or a SIP correction's Jacobian is smooth, and over a registration it moves by
/// parts in 10⁴ across the frame, so between nodes 32 px apart its largest singular value exceeds
/// the nodes' by a second-order fraction of that.
const STRETCH_GRID: usize = 32;

/// The weight below which a tap is left out of a window: `2⁻²⁷`, a sixteenth of f32's step at 1.
/// The band it cuts from the kernel's ends is under a source pixel wide at any stretch below 2800,
/// so an axis drops at most a tap at each end. An axis's weight sum is about its stretch, so at
/// least near 1, and its `Σ|w|` stays under twice that sum: an axis drops under `2·2·2⁻²⁷` of the
/// window's weight sum, the two together under `2⁻²⁴`, half of f32's step at 1.
const NEGLIGIBLE_WEIGHT: f64 = 1.0 / (1u32 << 27) as f64;

/// A separable filter the warp offers: every method but Nearest, which reads one pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Filter {
    Bilinear,
    Bicubic,
    Lanczos(LanczosOrder),
}

impl Filter {
    /// The filter `method` names; `None` for Nearest.
    pub(crate) const fn of(method: InterpolationMethod) -> Option<Self> {
        match method {
            InterpolationMethod::Nearest => None,
            InterpolationMethod::Bilinear => Some(Self::Bilinear),
            InterpolationMethod::Bicubic => Some(Self::Bicubic),
            InterpolationMethod::Lanczos2 => Some(Self::Lanczos(LanczosOrder::Two)),
            InterpolationMethod::Lanczos3 => Some(Self::Lanczos(LanczosOrder::Three)),
            InterpolationMethod::Lanczos4 => Some(Self::Lanczos(LanczosOrder::Four)),
        }
    }

    /// The distance at which the unstretched kernel reaches zero.
    const fn radius(self) -> usize {
        match self {
            Self::Bilinear => 1,
            Self::Bicubic => 2,
            Self::Lanczos(order) => order.a(),
        }
    }

    /// How far inside the radius the unstretched kernel's magnitude falls under
    /// [`NEGLIGIBLE_WEIGHT`] `w`, so that a stretch a hair above 1 keeps the window.
    /// - Lanczos-`a` at `a − ε`: each sinc factor is under `ε/(a − ε)`, so the kernel is under `w`
    ///   from `ε = a·√w/(1 + √w)`, 2.6e-4 for Lanczos3.
    /// - Catmull-Rom at `2 − ε`: `−½(1 − ε)ε²`, under `w` from `ε = √(2w)`, 1.2e-4.
    /// - Bilinear at `1 − ε`: `ε` itself.
    fn edge_tolerance(self) -> f64 {
        let w = NEGLIGIBLE_WEIGHT;
        match self {
            Self::Bilinear => w,
            Self::Bicubic => (2.0 * w).sqrt(),
            Self::Lanczos(order) => order.a() as f64 * w.sqrt() / (1.0 + w.sqrt()),
        }
    }

    /// Whether some tap can weigh less than zero: what the ringing clamp acts on.
    pub(crate) const fn has_negative_lobes(self) -> bool {
        !matches!(self, Self::Bilinear)
    }
}

/// A [`Filter`] at the stretch one frame's warp needs.
///
/// Where the warp shrinks the frame — an output pixel spans more than one source pixel — the source
/// holds frequencies past the output's Nyquist limit, and an unstretched kernel folds them back as
/// aliases. Widening the kernel by the scale lowers its cut-off by the same factor, the prefilter
/// every minifying resampler applies (DeForest 2004, "On re-sampling of solar images"; astropy's
/// `reproject` adaptive mode): the stretch is the largest singular value of the output-to-source
/// Jacobian, held to at least 1, so a warp that keeps or enlarges the scale leaves the kernel as it
/// is. One stretch serves the whole frame, at the largest scale on it: a per-pixel stretch would
/// move every window's reach, which the masked sources read once per frame to mark where a window
/// can meet a flagged pixel. The stretch is isotropic, so a warp that shrinks one axis only blurs
/// the other too.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WarpKernel {
    filter: Filter,
    lut: Option<&'static LanczosLut>,
    stretch: f32,
    inverse_stretch: f32,
    /// The table's entries per unit of stretched distance.
    lut_scale: f32,
    /// `(radius − edge tolerance) · stretch`: a tap this far from the sample or farther weighs
    /// nothing.
    reach: f32,
}

/// One axis of an output pixel's window: the source coordinate of its first tap, and each tap's
/// weight. The weights are the kernel's own, not normalized.
#[derive(Debug)]
pub(crate) struct TapAxis {
    pub(crate) start: i32,
    len: usize,
    /// The weights, then at least [`F32_LANES`] more slots, so a vector of weights loads or stores
    /// whole from any tap. Grown once to the widest window a frame needs, never shrunk.
    storage: Vec<f32>,
}

impl Default for TapAxis {
    fn default() -> Self {
        Self {
            start: 0,
            len: 0,
            storage: vec![0.0; F32_LANES],
        }
    }
}

impl TapAxis {
    pub(crate) fn weights(&self) -> &[f32] {
        &self.storage[..self.len]
    }

    /// The weights from tap `from` on, with the slack after them: what a vector load starts at.
    pub(crate) fn padded_from(&self, from: usize) -> &[f32] {
        &self.storage[from..]
    }

    /// `len` taps from `start`, their slots to be written.
    fn reset(&mut self, start: i32, len: usize) {
        if self.storage.len() < len + F32_LANES {
            self.storage.resize(len + F32_LANES, 0.0);
        }
        self.start = start;
        self.len = len;
    }
}

/// One axis's taps for a sample at `cell + frac`: the first, relative to the cell, and how many.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TapRange {
    pub(crate) first: i32,
    pub(crate) count: usize,
}

/// Lane indices, the tap offsets of one vector of weights.
const LANE_INDEX: [f32; F32_LANES] = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];

impl WarpKernel {
    /// `filter` widened by `stretch`, which is at least 1.
    pub(crate) fn new(filter: Filter, stretch: f32) -> Self {
        debug_assert!(
            stretch >= 1.0 && stretch.is_finite(),
            "a kernel stretch is at least 1, got {stretch}"
        );
        Self {
            filter,
            lut: match filter {
                Filter::Lanczos(order) => Some(order.lut()),
                Filter::Bilinear | Filter::Bicubic => None,
            },
            stretch,
            inverse_stretch: 1.0 / stretch,
            lut_scale: LANCZOS_LUT_RESOLUTION as f32 / stretch,
            reach: ((filter.radius() as f64 - filter.edge_tolerance()) * f64::from(stretch)) as f32,
        }
    }

    /// `filter` at the stretch `warp` needs over an output frame of `size`, the source's own.
    pub(crate) fn for_frame(filter: Filter, warp: &WarpTransform, size: Size2us) -> Self {
        Self::new(filter, Self::frame_stretch(warp, size))
    }

    /// The largest local scale of `warp` over the output pixels that land in the source, held to
    /// at least one. An affine map has one Jacobian; any other is read on [`STRETCH_GRID`] and the
    /// frame's last row and column. A node that lands outside the source has nothing to alias and
    /// is left out: near a homography's horizon the scale grows without bound.
    fn frame_stretch(warp: &WarpTransform, size: Size2us) -> f32 {
        let largest =
            if !warp.has_sip() && warp.transform.transform_type() != TransformType::Homography {
                largest_singular_value(warp.jacobian(DVec2::ZERO))
            } else {
                let nodes = |length: usize| {
                    (0..length)
                        .step_by(STRETCH_GRID)
                        .chain((length % STRETCH_GRID != 1).then_some(length - 1))
                };
                nodes(size.height)
                    .flat_map(|y| nodes(size.width).map(move |x| DVec2::new(x as f64, y as f64)))
                    .filter(|&p| SourcePosition::within(warp.apply(p), size).is_some())
                    .map(|p| largest_singular_value(warp.jacobian(p)))
                    .fold(1.0, f64::max)
            };
        largest.max(1.0) as f32
    }

    /// Bilinear at this kernel's stretch: the fallback where this kernel's surviving taps cannot
    /// be trusted to normalize.
    pub(crate) fn bilinear(self) -> Self {
        Self::new(Filter::Bilinear, self.stretch)
    }

    /// The taps any window of this kernel reads around its sample's cell: from
    /// `cell − (⌈reach⌉ − 1)` to `cell + ⌈reach⌉`.
    #[expect(
        clippy::cast_sign_loss,
        reason = "a reach is a positive radius times a stretch of at least 1"
    )]
    pub(crate) const fn window_reach(self) -> Reach {
        let reach = self.reach.ceil() as usize;
        Reach {
            before: reach - 1,
            after: reach,
        }
    }

    /// The taps of the sample at `cell + frac` on one axis, `frac` in `[0, 1]`.
    ///
    /// Unstretched, the window is the kernel's `2·radius` taps from `cell − (radius − 1)`, a tap at
    /// or past the reach weighing zero. Stretched, it is every source coordinate whose distance,
    /// as [`Self::weight_lanes`] rounds it, is within the reach, which leaves such a tap out. The
    /// bounds from `frac ± reach` round apart from those distances by at most a tap, which the
    /// distances themselves then settle.
    #[inline(always)]
    pub(crate) fn taps(&self, frac: f32) -> TapRange {
        let (first, last) = if self.stretch == 1.0 {
            let radius = self.filter.radius() as i32;
            (1 - radius, radius)
        } else {
            let within = |t: i32| (t as f32 - frac).abs() < self.reach;
            let first = floor(frac - self.reach) + 1;
            let last = -floor(-(frac + self.reach)) - 1;
            (
                first + i32::from(!within(first)) - i32::from(within(first - 1)),
                last + i32::from(within(last + 1)) - i32::from(!within(last)),
            )
        };
        TapRange {
            first,
            count: (last - first + 1).unsigned_abs() as usize,
        }
    }

    /// The weights of the [`F32_LANES`] taps from `cell + first` on, for a sample at `cell + frac`:
    /// the kernel at each tap's distance, and zero at or past the reach.
    ///
    /// A tap's distance is `|t − frac|`, which rounds as `−t + frac` at or below the cell and
    /// `t − frac` above it.
    #[inline(always)]
    pub(crate) fn weight_lanes<S: Isa>(&self, isa: S, first: i32, frac: f32) -> S::F32 {
        let signed = isa.load_f32(&LANE_INDEX) + isa.splat_f32(first as f32) - isa.splat_f32(frac);
        let distance = signed.max(isa.splat_f32(0.0) - signed);
        let weight = match self.filter {
            Filter::Lanczos(_) => {
                let lut = self.lut.expect("a Lanczos kernel holds its table");
                // The scalar `LanczosLut::at` read, lane by lane.
                let position = distance * isa.splat_f32(self.lut_scale);
                let below = position.floor();
                let low = isa.lookup_f32(&lut.values, below);
                let high = isa.lookup_f32(&lut.values, below + isa.splat_f32(1.0));
                (high - low).mul_add(position - below, low)
            }
            Filter::Bicubic => bicubic_lanes(isa, distance * isa.splat_f32(self.inverse_stretch)),
            Filter::Bilinear => (isa.splat_f32(1.0)
                - distance * isa.splat_f32(self.inverse_stretch))
            .max(isa.splat_f32(0.0)),
        };
        distance.lanes_lt(isa.splat_f32(self.reach)).keep(weight)
    }

    /// The taps of the sample at `cell + frac` and their weights, into `axis`.
    #[inline(always)]
    pub(crate) fn axis<S: Isa>(&self, isa: S, cell: i32, frac: f32, axis: &mut TapAxis) {
        let TapRange { first, count } = self.taps(frac);
        axis.reset(cell + first, count);
        for (chunk, offset) in (0..count)
            .step_by(F32_LANES)
            .zip((first..).step_by(F32_LANES))
        {
            self.weight_lanes(isa, offset, frac).store(
                (&mut axis.storage[chunk..chunk + F32_LANES])
                    .try_into()
                    .expect("the storage holds a vector past the weights"),
            );
        }
    }
}

/// Catmull-Rom (`A = −½`) at non-negative distances, each lane in the operation order of the scalar
/// test reference, `kernel::internals::bicubic_kernel`.
#[inline(always)]
fn bicubic_lanes<S: Isa>(isa: S, x: S::F32) -> S::F32 {
    const A: f32 = -0.5;
    let splat = |value: f32| isa.splat_f32(value);
    let inner = (splat(A + 2.0) * x - splat(A + 3.0)) * x * x + splat(1.0);
    let outer = ((splat(A) * x - splat(5.0 * A)) * x + splat(8.0 * A)) * x - splat(4.0 * A);
    let near = x.lanes_gt(splat(1.0));
    let far = x.lanes_lt(splat(2.0));
    let beyond_one = near.select(outer, inner);
    far.keep(beyond_one)
}

/// `x.floor()` for a finite `x` within `i32` range: truncation, then one step down for a negative
/// non-integer.
#[inline]
fn floor(x: f32) -> i32 {
    let truncated = x as i32;
    truncated - i32::from(x < truncated as f32)
}

/// The largest singular value of `m`: the square root of the larger eigenvalue of `mᵀm`, whose
/// trace is the squared Frobenius norm `F` and whose determinant is `det(m)²`.
fn largest_singular_value(m: DMat2) -> f64 {
    let frobenius = m.x_axis.length_squared() + m.y_axis.length_squared();
    let determinant = m.determinant();
    let discriminant = (frobenius * frobenius - 4.0 * determinant * determinant).max(0.0);
    f64::midpoint(frobenius, discriminant.sqrt()).sqrt()
}

#[cfg(test)]
mod tests;
