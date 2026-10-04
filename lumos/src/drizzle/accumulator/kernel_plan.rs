//! [`KernelPlan`]: what a drizzle kernel needs beyond the frame, resolved once per run.

use crate::drizzle::config::{DrizzleConfig, DrizzleKernel};
use crate::math::fwhm::FWHM_PER_SIGMA;

/// Output rows of slack on every kernel's input-row estimate. `OutputBand::deposit_rows` rounds a
/// drop's extent to the nearest row, so a drop stopping half a row short of the band still reaches
/// it; nothing else separates a drop's centre from the rows it touches.
const ROW_ROUNDING_SLACK: f64 = 0.5;
/// Where the Gaussian is truncated, in σ — it has fallen to ~1% of its peak by there.
const GAUSSIAN_RADIUS_SIGMAS: f64 = 3.0;
/// The drizzle Lanczos kernel is `STScI`'s Lanczos-3: support radius 3, defined on [-3, 3].
pub(super) const LANCZOS_A: f32 = 3.0;

/// Everything a kernel needs beyond the frame, resolved once per run.
///
/// Every field is a function of [`DrizzleConfig`] alone, so resolving it here is what keeps a band
/// from recomputing an `exp`'s σ, or a drop's area, on each of the thousands of frames × bands the
/// scatter is split into.
#[derive(Debug, Clone, Copy)]
pub(super) enum KernelPlan {
    /// Half the drop's side, in *input* pixels: the square kernel shrinks the input pixel and then
    /// maps its corners.
    Square {
        half_drop: f64,
    },
    /// Half the drop's side and the reciprocal of its area, in *output* pixels.
    Turbo {
        half_drop: f64,
        inv_area: f64,
    },
    Point,
    Gaussian {
        radius: isize,
        inv_2sigma_sq: f32,
    },
    Lanczos {
        radius: isize,
    },
}

impl KernelPlan {
    pub(super) fn new(config: &DrizzleConfig) -> Self {
        // Drop size in output pixels: pixfrac is the fraction of input pixel size, and each input
        // pixel maps to `scale` output pixels, so drop = pixfrac · scale. (STScI: pfo =
        // pixel_fraction / pscale_ratio / 2, where pscale_ratio = 1/scale.)
        let drop_size = f64::from(config.pixfrac) * f64::from(config.scale);
        match config.kernel {
            DrizzleKernel::Square => Self::Square {
                half_drop: 0.5 * f64::from(config.pixfrac),
            },
            DrizzleKernel::Turbo => Self::Turbo {
                half_drop: 0.5 * drop_size,
                inv_area: 1.0 / (drop_size * drop_size),
            },
            DrizzleKernel::Point => Self::Point,
            DrizzleKernel::Gaussian => {
                // Per `STScI` the Gaussian's FWHM is the drop size.
                let sigma = drop_size / FWHM_PER_SIGMA;
                Self::Gaussian {
                    radius: (GAUSSIAN_RADIUS_SIGMAS * sigma).ceil() as isize,
                    inv_2sigma_sq: (1.0 / (2.0 * sigma * sigma)) as f32,
                }
            }
            DrizzleKernel::Lanczos => Self::Lanczos {
                radius: LANCZOS_A as isize,
            },
        }
    }

    /// How far a drop reaches from its pixel's centre: in output rows, and in input rows — the
    /// square kernel's drop is a box in the input, mapped corner by corner. Each includes
    /// [`ROW_ROUNDING_SLACK`] where the deposit rounds an output extent to rows.
    pub(super) fn reach(self) -> DropReach {
        match self {
            Self::Square { half_drop } => DropReach {
                output_rows: ROW_ROUNDING_SLACK,
                input_rows: half_drop,
            },
            Self::Turbo { half_drop, .. } => DropReach {
                output_rows: half_drop + ROW_ROUNDING_SLACK,
                input_rows: 0.0,
            },
            Self::Point => DropReach {
                output_rows: ROW_ROUNDING_SLACK,
                input_rows: 0.0,
            },
            Self::Gaussian { radius, .. } | Self::Lanczos { radius } => DropReach {
                output_rows: radius as f64 + ROW_ROUNDING_SLACK,
                input_rows: 0.0,
            },
        }
    }
}

/// How far a drop reaches from its pixel's centre, in the two grids — see [`KernelPlan::reach`].
#[derive(Debug, Clone, Copy)]
pub(super) struct DropReach {
    pub(super) output_rows: f64,
    pub(super) input_rows: f64,
}
