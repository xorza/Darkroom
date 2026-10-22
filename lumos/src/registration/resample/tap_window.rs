//! [`TapWindow`]: the taps of one output pixel that land in the source, and the sums a sample and
//! its quality are made of.
//!
//! A window row is one vector per [`F32_LANES`] taps, zero-padded past the last, so a row reads
//! exactly its own pixels. Every Isa folds the products in one order, so a sample is the same bits
//! on every CPU.

use crate::math::size2us::Size2us;
use crate::registration::resample::flagged_sources::Validity;
use crate::registration::resample::ringing_clamp::{LobeSums, LobeWeights};
use crate::registration::resample::source_image::SourcePlane;
use crate::registration::resample::warp_kernel::TapAxis;
use crate::simd::{F32_LANES, F32x8, Isa, Mask8};

/// Weight sums over the taps of a window that hold data, the weight of a tap being `L = wₓ·w_y`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct WindowWeights {
    /// `Σ L` over the taps with `L > 0`.
    pub(crate) positive: f32,
    /// `Σ L` over the taps with `L < 0`: zero or less.
    pub(crate) negative: f32,
    /// `Σ L²`.
    pub(crate) square: f32,
}

impl WindowWeights {
    pub(crate) fn total(self) -> f32 {
        self.positive + self.negative
    }

    pub(crate) const fn lobes(self) -> LobeWeights {
        LobeWeights {
            positive: self.positive,
            negative: self.negative,
        }
    }

    /// `Σ |L|`.
    pub(crate) fn magnitude(self) -> f32 {
        self.positive - self.negative
    }

    /// Whether these taps, normalized by their sum, sample no noisier than one source pixel:
    /// `(Σ L)² ≥ Σ L²` with `Σ L > 0`.
    ///
    /// Normalized convolution is a weighted mean only while its applicability is positive (Knutsson
    /// and Westin 1993); a signed kernel cut down to a few taps can sum to nearly nothing, or below
    /// zero, and dividing by that amplifies the noise without bound. The white-noise variance of a
    /// normalized sample is `Σ L² / (Σ L)²` times a pixel's, so the test admits exactly the windows
    /// that do not raise it past one pixel. A whole kernel passes at every fraction — at an integer
    /// one it is the centre tap alone, at the boundary — so the test only decides the windows the
    /// source edge or a null has cut.
    pub(super) fn well_conditioned(self) -> bool {
        let total = self.total();
        total > 0.0 && total * total >= self.square
    }

    /// Kish's effective sample size `(Σ L)² / Σ L²`: how many equally weighted pixels the
    /// normalized weights are worth, the inverse of the white-noise variance they leave.
    pub(crate) fn confidence(self) -> f32 {
        let total = self.total();
        total * total / self.square
    }
}

/// One axis's weights summed by sign, and their squares.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AxisSums {
    positive: f32,
    negative: f32,
    square: f32,
}

impl AxisSums {
    fn of(weights: &[f32]) -> Self {
        let mut sums = Self::default();
        for &weight in weights {
            if weight > 0.0 {
                sums.positive += weight;
            } else {
                sums.negative += weight;
            }
            sums.square += weight * weight;
        }
        sums
    }

    /// The sums of one vector of weights, its lanes past the taps cleared.
    #[inline(always)]
    pub(super) fn of_lanes<S: Isa>(isa: S, weights: S::F32) -> Self {
        let zero = isa.splat_f32(0.0);
        Self {
            positive: weights.max(zero).reduce_sum(),
            negative: weights.min(zero).reduce_sum(),
            square: (weights * weights).reduce_sum(),
        }
    }

    /// The weight sums of the window whose taps weigh `L = wₓ·w_y` over these x sums and `y`'s:
    /// products of the axes' own sums, since `L` takes the sign of the two factors together.
    pub(crate) fn window(self, y: Self) -> WindowWeights {
        WindowWeights {
            positive: self.positive * y.positive + self.negative * y.negative,
            negative: self.positive * y.negative + self.negative * y.positive,
            square: self.square * y.square,
        }
    }
}

/// Products on one window row's taps, split by the sign of their x weight, or the window's sums of
/// them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Lobes<V> {
    pub(crate) positive: V,
    pub(crate) negative: V,
}

impl<V: F32x8> Lobes<V> {
    #[inline(always)]
    pub(crate) fn zero<S: Isa<F32 = V>>(isa: S) -> Self {
        Self {
            positive: isa.splat_f32(0.0),
            negative: isa.splat_f32(0.0),
        }
    }

    /// `row` carried into these sums by its weight `wy`: a negative `wy` turns each split's sign,
    /// so each goes to the other lobe.
    #[inline(always)]
    pub(super) fn add_row<S: Isa<F32 = V>>(&mut self, isa: S, row: Self, wy: f32) {
        let (to_positive, to_negative) = if wy > 0.0 {
            (row.positive, row.negative)
        } else {
            (row.negative, row.positive)
        };
        let wy = isa.splat_f32(wy);
        self.positive = to_positive.mul_add(wy, self.positive);
        self.negative = to_negative.mul_add(wy, self.negative);
    }
}

/// One output pixel's window clipped to the source: where its first in-bounds tap is, and the
/// weights of the in-bounds taps on each axis.
#[derive(Debug)]
pub(crate) struct TapWindow<'a> {
    x: usize,
    y: usize,
    wx: &'a [f32],
    wy: &'a [f32],
    /// `wx` and the slots after it, at least a vector's worth.
    wx_padded: &'a [f32],
    whole_x: &'a [f32],
    whole_y: &'a [f32],
}

/// Lane indices, for the mask that keeps a row's first vector to the window's taps.
const LANE_IDX: [f32; F32_LANES] = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];

/// The weights of one chunk of a window row, split by sign as the lobe sums take them.
#[derive(Debug, Clone, Copy)]
struct SignedLanes<V> {
    all: V,
    positive: V,
    negative: V,
}

impl<V: F32x8> SignedLanes<V> {
    #[inline(always)]
    fn load<S: Isa<F32 = V>>(isa: S, weights: &[f32]) -> Self {
        let all = isa.load_f32_partial(weights);
        let zero = isa.splat_f32(0.0);
        Self {
            all,
            positive: all.max(zero),
            negative: all.min(zero),
        }
    }
}

/// Clipping of one axis: the first in-bounds tap's coordinate and the range of its weights.
#[derive(Debug, Clone, Copy)]
struct Clip {
    first: usize,
    from: usize,
    to: usize,
}

impl Clip {
    /// `None` when no tap of `axis` lands in `[0, length)`.
    #[expect(
        clippy::cast_sign_loss,
        reason = "both ends are clamped to the image, so they are non-negative"
    )]
    fn of(axis: &TapAxis, length: usize) -> Option<Self> {
        let start = i64::from(axis.start);
        let from = (-start).max(0);
        let to = (length as i64 - start).min(axis.weights().len() as i64);
        (from < to).then(|| Self {
            first: (start + from) as usize,
            from: from as usize,
            to: to as usize,
        })
    }
}

impl<'a> TapWindow<'a> {
    /// The window of `x` and `y` in a `size` source; `None` when it misses the source on an axis.
    pub(crate) fn new(x: &'a TapAxis, y: &'a TapAxis, size: Size2us) -> Option<Self> {
        let clip_x = Clip::of(x, size.width)?;
        let clip_y = Clip::of(y, size.height)?;
        Some(Self {
            x: clip_x.first,
            y: clip_y.first,
            wx: &x.weights()[clip_x.from..clip_x.to],
            wy: &y.weights()[clip_y.from..clip_y.to],
            wx_padded: x.padded_from(clip_x.from),
            whole_x: x.weights(),
            whole_y: y.weights(),
        })
    }

    /// Whether the source edge cut some tap of the kernel.
    pub(super) const fn is_clipped(&self) -> bool {
        self.wx.len() < self.whole_x.len() || self.wy.len() < self.whole_y.len()
    }

    /// `Σ |L|` over the whole kernel, in bounds or not.
    pub(super) fn whole_magnitude(&self) -> f32 {
        let magnitude = |weights: &[f32]| weights.iter().map(|weight| weight.abs()).sum::<f32>();
        magnitude(self.whole_x) * magnitude(self.whole_y)
    }

    /// The weight sums when every in-bounds tap holds data ([`AxisSums::window`]).
    pub(crate) fn weights(&self) -> WindowWeights {
        AxisSums::of(self.wx).window(AxisSums::of(self.wy))
    }

    /// The weight sums over the taps that `validity` holds: a source with nulls, where the window
    /// is no longer a product of its axes.
    #[inline(always)]
    pub(super) fn masked_weights<S: Isa>(&self, isa: S, validity: Validity<'_>) -> WindowWeights {
        let mut lobes = Lobes::zero(isa);
        let mut square = isa.splat_f32(0.0);
        let head = self.head(isa);
        let first = head.weights;
        let first_square = first.all * first.all;
        for (j, &wy) in self.wy.iter().enumerate() {
            let row = self.row(validity.width, j);
            let valid = head.valid(isa, validity, row);
            let mut on_row = Lobes {
                positive: valid * first.positive,
                negative: valid * first.negative,
            };
            let mut on_square = valid * first_square;
            for start in (F32_LANES..self.wx.len()).step_by(F32_LANES) {
                let end = (start + F32_LANES).min(self.wx.len());
                let lanes = SignedLanes::load(isa, &self.wx[start..end]);
                let valid = validity.lanes(isa, row + start, end - start);
                on_row.positive = valid.mul_add(lanes.positive, on_row.positive);
                on_row.negative = valid.mul_add(lanes.negative, on_row.negative);
                on_square = (valid * lanes.all).mul_add(lanes.all, on_square);
            }
            lobes.add_row(isa, on_row, wy);
            square = on_square.mul_add(isa.splat_f32(wy * wy), square);
        }
        WindowWeights {
            positive: lobes.positive.reduce_sum(),
            negative: lobes.negative.reduce_sum(),
            square: square.reduce_sum(),
        }
    }

    /// `Σ L·f` over the window of `plane`, over the taps `validity` holds when it is given.
    #[inline(always)]
    pub(crate) fn total<S: Isa>(
        &self,
        isa: S,
        plane: SourcePlane<'_>,
        validity: Option<Validity<'_>>,
    ) -> f32 {
        let head = self.head(isa);
        let first = head.weights.all;
        let mut sum = isa.splat_f32(0.0);
        for (j, &wy) in self.wy.iter().enumerate() {
            let row = self.row(plane.width, j);
            let values = head.load(isa, plane.pixels, row);
            let mut on_row =
                valid_only(isa, values, validity.map(|v| head.valid(isa, v, row))) * first;
            for start in (F32_LANES..self.wx.len()).step_by(F32_LANES) {
                let end = (start + F32_LANES).min(self.wx.len());
                let values = isa.load_f32_partial(&plane.pixels[row + start..row + end]);
                let valid = validity.map(|v| v.lanes(isa, row + start, end - start));
                on_row = valid_only(isa, values, valid)
                    .mul_add(isa.load_f32_partial(&self.wx[start..end]), on_row);
            }
            sum = on_row.mul_add(isa.splat_f32(wy), sum);
        }
        sum.reduce_sum()
    }

    /// The window of `plane` summed as [`LobeSums`] reads it, over the taps `validity` holds when
    /// it is given.
    #[inline(always)]
    pub(super) fn lobe_sums<S: Isa>(
        &self,
        isa: S,
        plane: SourcePlane<'_>,
        validity: Option<Validity<'_>>,
    ) -> LobeSums {
        let zero = isa.splat_f32(0.0);
        let mut lobes = Lobes::zero(isa);
        let mut below_zero = zero;
        let head = self.head(isa);
        let first = head.weights;
        for (j, &wy) in self.wy.iter().enumerate() {
            let row = self.row(plane.width, j);
            let values = valid_only(
                isa,
                head.load(isa, plane.pixels, row),
                validity.map(|v| head.valid(isa, v, row)),
            );
            let light = values.max(zero);
            let mut on_row = Lobes {
                positive: light * first.positive,
                negative: light * first.negative,
            };
            let mut on_below = (values - light) * first.all;
            for start in (F32_LANES..self.wx.len()).step_by(F32_LANES) {
                let end = (start + F32_LANES).min(self.wx.len());
                let lanes = SignedLanes::load(isa, &self.wx[start..end]);
                let values = valid_only(
                    isa,
                    isa.load_f32_partial(&plane.pixels[row + start..row + end]),
                    validity.map(|v| v.lanes(isa, row + start, end - start)),
                );
                let light = values.max(zero);
                on_row.positive = light.mul_add(lanes.positive, on_row.positive);
                on_row.negative = light.mul_add(lanes.negative, on_row.negative);
                on_below = (values - light).mul_add(lanes.all, on_below);
            }
            lobes.add_row(isa, on_row, wy);
            below_zero = on_below.mul_add(isa.splat_f32(wy), below_zero);
        }
        LobeSums {
            positive: lobes.positive.reduce_sum(),
            negative: lobes.negative.reduce_sum(),
            below_zero: below_zero.reduce_sum(),
        }
    }

    /// The first vector of every row: its weights, and the mask that keeps it to the window.
    #[inline(always)]
    fn head<S: Isa>(&self, isa: S) -> Head<S::F32> {
        let lanes = RowLanes::first(isa, self.wx.len().min(F32_LANES));
        let weights = lanes.keep().keep(
            isa.load_f32(
                self.wx_padded[..F32_LANES]
                    .try_into()
                    .expect("an axis keeps a vector of slots past its weights"),
            ),
        );
        let zero = isa.splat_f32(0.0);
        Head {
            weights: SignedLanes {
                all: weights,
                positive: weights.max(zero),
                negative: weights.min(zero),
            },
            lanes,
        }
    }

    /// Where row `j` of the window starts in a plane `width` wide, the source's.
    #[inline(always)]
    const fn row(&self, width: usize, j: usize) -> usize {
        (self.y + j) * width + self.x
    }
}

/// `values` where `valid` holds data, zero elsewhere — selected, not multiplied, so a NaN under an
/// excluded tap never reaches a sum — or `values` whole for a source that excludes none.
#[inline(always)]
fn valid_only<S: Isa>(isa: S, values: S::F32, valid: Option<S::F32>) -> S::F32 {
    match valid {
        Some(valid) => valid.lanes_gt(isa.splat_f32(0.0)).keep(values),
        None => values,
    }
}

/// [`TapWindow::head`]: the weights of a row's first vector, split by sign, and the lanes they
/// occupy.
#[derive(Debug, Clone, Copy)]
struct Head<V: F32x8> {
    weights: SignedLanes<V>,
    lanes: RowLanes<V>,
}

impl<V: F32x8> Head<V> {
    #[inline(always)]
    fn load<S: Isa<F32 = V>>(self, isa: S, pixels: &[f32], row: usize) -> V {
        self.lanes.load(isa, pixels, row)
    }

    /// The [`Validity::lanes`] of the row starting at `row`, past the window cleared.
    #[inline(always)]
    fn valid<S: Isa<F32 = V>>(self, isa: S, validity: Validity<'_>, row: usize) -> V {
        validity.lanes(isa, row, self.lanes.count)
    }
}

/// The lanes of a window row's first vector: the mask that keeps the row's taps, and their count.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RowLanes<V: F32x8> {
    keep: V::Mask,
    count: usize,
}

impl<V: F32x8> RowLanes<V> {
    /// The first `count` lanes, at most [`F32_LANES`].
    #[inline(always)]
    pub(crate) fn first<S: Isa<F32 = V>>(isa: S, count: usize) -> Self {
        Self {
            keep: isa
                .load_f32(&LANE_IDX)
                .lanes_lt(isa.splat_f32(count as f32)),
            count,
        }
    }

    /// The first vector of the row starting at `row` in `pixels`: one whole load with the lanes
    /// past the window cleared — so a pixel beside the window, whatever it holds, adds nothing —
    /// or, at the end of the plane where a whole vector would run past it, a partial one.
    #[inline(always)]
    pub(crate) fn load<S: Isa<F32 = V>>(self, isa: S, pixels: &[f32], row: usize) -> V {
        match pixels.get(row..row + F32_LANES) {
            Some(whole) => self
                .keep
                .keep(isa.load_f32(whole.try_into().expect("a slice of a vector's length"))),
            None => isa.load_f32_partial(&pixels[row..row + self.count]),
        }
    }

    pub(crate) const fn keep(self) -> V::Mask {
        self.keep
    }
}
