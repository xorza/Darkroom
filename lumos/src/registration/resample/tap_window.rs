//! [`TapWindow`]: the taps of one output pixel that land in the source, and the sums a sample and
//! its quality are made of.
//!
//! A window row is one vector per [`F32_LANES`] taps, zero-padded past the last, so a row reads
//! exactly its own pixels. Every Isa folds the products in one order, so a sample is the same bits
//! on every CPU.

use imaginarium::Buffer2;

use crate::math::size2us::Size2us;
use crate::registration::resample::kernel::warp_kernel::TapAxis;
use crate::registration::resample::ringing_clamp::{LobeSums, LobeWeights};
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
    pub(crate) fn well_conditioned(self) -> bool {
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

/// One axis's weights summed by sign.
#[derive(Debug, Clone, Copy, Default)]
struct AxisSums {
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
const LANE_INDEX: [f32; F32_LANES] = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];

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
    pub(crate) const fn is_clipped(&self) -> bool {
        self.wx.len() < self.whole_x.len() || self.wy.len() < self.whole_y.len()
    }

    /// `Σ |L|` over the whole kernel, in bounds or not.
    pub(crate) fn whole_magnitude(&self) -> f32 {
        let magnitude = |weights: &[f32]| weights.iter().map(|weight| weight.abs()).sum::<f32>();
        magnitude(self.whole_x) * magnitude(self.whole_y)
    }

    /// The weight sums when every in-bounds tap holds data: products of the axes' own sums, since
    /// `L = wₓ·w_y` takes the sign of the two factors together.
    pub(crate) fn weights(&self) -> WindowWeights {
        let x = AxisSums::of(self.wx);
        let y = AxisSums::of(self.wy);
        WindowWeights {
            positive: x.positive * y.positive + x.negative * y.negative,
            negative: x.positive * y.negative + x.negative * y.positive,
            square: x.square * y.square,
        }
    }

    /// The weight sums over the taps `validity` marks with 1, the others 0: a source with nulls,
    /// where the window is no longer a product of its axes.
    #[inline(always)]
    pub(crate) fn masked_weights<S: Isa>(&self, isa: S, validity: &Buffer2<f32>) -> WindowWeights {
        let zero = isa.splat_f32(0.0);
        let (mut positive, mut negative, mut square) = (zero, zero, zero);
        let head = self.head(isa);
        let first = head.weights;
        let first_square = first.all * first.all;
        for (j, &wy) in self.wy.iter().enumerate() {
            let row = self.row(validity, j);
            let valid = head.load(isa, validity.pixels(), row);
            let mut on_positive = valid * first.positive;
            let mut on_negative = valid * first.negative;
            let mut on_square = valid * first_square;
            for start in (F32_LANES..self.wx.len()).step_by(F32_LANES) {
                let end = (start + F32_LANES).min(self.wx.len());
                let lanes = SignedLanes::load(isa, &self.wx[start..end]);
                let valid = isa.load_f32_partial(&validity.pixels()[row + start..row + end]);
                on_positive = valid.mul_add(lanes.positive, on_positive);
                on_negative = valid.mul_add(lanes.negative, on_negative);
                on_square = (valid * lanes.all).mul_add(lanes.all, on_square);
            }
            let wy_lanes = isa.splat_f32(wy);
            let (to_positive, to_negative) = if wy > 0.0 {
                (on_positive, on_negative)
            } else {
                (on_negative, on_positive)
            };
            positive = to_positive.mul_add(wy_lanes, positive);
            negative = to_negative.mul_add(wy_lanes, negative);
            square = on_square.mul_add(isa.splat_f32(wy * wy), square);
        }
        WindowWeights {
            positive: positive.reduce_sum(),
            negative: negative.reduce_sum(),
            square: square.reduce_sum(),
        }
    }

    /// `Σ L·f` over the window of `plane`.
    #[inline(always)]
    pub(crate) fn total<S: Isa>(&self, isa: S, plane: &Buffer2<f32>) -> f32 {
        let head = self.head(isa);
        let first = head.weights.all;
        let mut sum = isa.splat_f32(0.0);
        for (j, &wy) in self.wy.iter().enumerate() {
            let row = self.row(plane, j);
            let mut on_row = head.load(isa, plane.pixels(), row) * first;
            for start in (F32_LANES..self.wx.len()).step_by(F32_LANES) {
                let end = (start + F32_LANES).min(self.wx.len());
                on_row = isa
                    .load_f32_partial(&plane.pixels()[row + start..row + end])
                    .mul_add(isa.load_f32_partial(&self.wx[start..end]), on_row);
            }
            sum = on_row.mul_add(isa.splat_f32(wy), sum);
        }
        sum.reduce_sum()
    }

    /// The window of `plane` summed as [`LobeSums`] reads it.
    #[inline(always)]
    pub(crate) fn lobe_sums<S: Isa>(&self, isa: S, plane: &Buffer2<f32>) -> LobeSums {
        let zero = isa.splat_f32(0.0);
        let (mut positive, mut negative, mut below_zero) = (zero, zero, zero);
        let head = self.head(isa);
        let first = head.weights;
        for (j, &wy) in self.wy.iter().enumerate() {
            let row = self.row(plane, j);
            let values = head.load(isa, plane.pixels(), row);
            let light = values.max(zero);
            let mut on_positive = light * first.positive;
            let mut on_negative = light * first.negative;
            let mut on_below = (values - light) * first.all;
            for start in (F32_LANES..self.wx.len()).step_by(F32_LANES) {
                let end = (start + F32_LANES).min(self.wx.len());
                let lanes = SignedLanes::load(isa, &self.wx[start..end]);
                let values = isa.load_f32_partial(&plane.pixels()[row + start..row + end]);
                let light = values.max(zero);
                on_positive = light.mul_add(lanes.positive, on_positive);
                on_negative = light.mul_add(lanes.negative, on_negative);
                on_below = (values - light).mul_add(lanes.all, on_below);
            }
            let wy_lanes = isa.splat_f32(wy);
            let (to_positive, to_negative) = if wy > 0.0 {
                (on_positive, on_negative)
            } else {
                (on_negative, on_positive)
            };
            positive = to_positive.mul_add(wy_lanes, positive);
            negative = to_negative.mul_add(wy_lanes, negative);
            below_zero = on_below.mul_add(wy_lanes, below_zero);
        }
        LobeSums {
            positive: positive.reduce_sum(),
            negative: negative.reduce_sum(),
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

    /// Where row `j` of the window starts in a plane of the source's width.
    #[inline(always)]
    const fn row(&self, plane: &Buffer2<f32>, j: usize) -> usize {
        (self.y + j) * plane.width() + self.x
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
                .load_f32(&LANE_INDEX)
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
