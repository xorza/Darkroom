//! [`Combine`]: how the samples at one pixel become one value, and its presets.

use crate::combine::rejection::Rejection;
use crate::combine::stack_config::combine_method::CombineMethod;
use crate::combine::stack_config::small_n::SmallN;

/// Default frames below which sigma-clip and linear-fit rejection are too unreliable to trust and
/// the combine falls back to the median. GESD has its own stricter floor. Winsorized clips once,
/// about an estimate of every sample, and trim measures no spread, so neither has one.
pub(super) const MIN_FRAMES_FOR_REJECTION: usize = 5;
pub(super) const MIN_FRAMES_FOR_GESD: usize = 15;

/// How the samples at one pixel become one value: the method, with its rejection, and the method
/// it falls back to below the frame count that rejection needs.
///
/// The constructors are the method presets. They set the method alone; the normalization and the
/// weighting belong to the role of the frames, which [`StackConfig`](crate::StackConfig)'s presets
/// carry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Combine {
    /// How to combine pixel values across frames. For `Mean`, includes the rejection algorithm.
    pub method: CombineMethod,
    /// Combine method used when there are too few frames for `method`'s rejection (see [`SmallN`]).
    pub small_n: SmallN,
}

impl Combine {
    /// Sigma-clipped mean, with the median below the frames sigma statistics need.
    pub const fn sigma_clip(sigma: f32) -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::sigma_clip(sigma)),
            small_n: SmallN::median_below(MIN_FRAMES_FOR_REJECTION),
        }
    }

    /// Median (implicit outlier rejection).
    pub const fn median() -> Self {
        Self {
            method: CombineMethod::Median,
            small_n: SmallN::none(),
        }
    }

    /// Plain mean without rejection.
    pub const fn mean() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::None),
            small_n: SmallN::none(),
        }
    }

    /// Winsorized sigma clipping, with no median fallback: it clips once, about a Huber estimate of
    /// every sample. On ten clean samples at k = 3 it rejects 0.12% under the frames' noise floor
    /// and 1.5% with none, against the Gaussian tail share of 0.27%.
    pub const fn winsorized(sigma: f32) -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::winsorized(sigma)),
            small_n: SmallN::none(),
        }
    }

    /// Linear fit clipping (good for sky gradients).
    pub const fn linear_fit(sigma: f32) -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::linear_fit(sigma)),
            small_n: SmallN::median_below(MIN_FRAMES_FOR_REJECTION),
        }
    }

    /// A trimmed mean, dropping `percent` of the samples from each end. A trim measures no spread,
    /// so a small stack needs no median fallback.
    pub const fn trim(percent: f32) -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::trim(percent)),
            small_n: SmallN::none(),
        }
    }

    /// Validation-constrained automatic GESD with a median fallback below its supported sample
    /// size.
    pub const fn gesd() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::gesd()),
            small_n: SmallN::median_below(MIN_FRAMES_FOR_GESD),
        }
    }
}
