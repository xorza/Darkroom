//! [`ColorData`]: what LibRaw reports of a file's levels and color once it is unpacked.

use crate::{CBLACK_LEN, CURVE_LEN};

/// What LibRaw reports of a file's black and white levels, its linearization and its white
/// balance, borrowed from LibRaw's state.
#[derive(Debug, Clone, Copy)]
pub struct ColorData<'a> {
    /// The common black level in ADU, as LibRaw's `adjust_bl` leaves it.
    pub black: u32,
    /// What each of the four color channels adds to `black`, then the spatial pattern's height
    /// and width, then the pattern.
    pub cblack: &'a [u32; CBLACK_LEN],
    /// The white level in ADU.
    pub maximum: u32,
    /// Each channel's linear limit the maker notes state, `0` where they state none.
    pub linear_max: [u32; 4],
    /// The camera's as-shot white balance multipliers, `[R, G1, B, G2]`.
    pub cam_mul: [f32; 4],
    /// Whether the samples already carry the as-shot white balance: a Sony YCC pseudo-RAW, a
    /// Nikon sRAW, an in-camera multi-exposure.
    pub as_shot_wb_applied: bool,
    /// A DNG's `BlackLevel`, before LibRaw rounds it into `black`.
    pub dng_black: f32,
    /// A DNG's per-channel and spatial black levels, before LibRaw rounds them into `cblack`.
    pub dng_cblack: &'a [f32; CBLACK_LEN],
    /// The sums of the masked pixels per color, then their counts.
    pub black_stat: [u32; 8],
    /// LibRaw's linearization curve, a value for each raw code.
    pub curve: &'a [u16; CURVE_LEN],
}
