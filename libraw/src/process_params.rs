//! [`ProcessParams`]: what LibRaw's own processing runs with.

/// What LibRaw's own processing runs with, of the parameters a caller sets: each field LibRaw's
/// `libraw_output_params_t` field of that name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcessParams {
    /// The output gamma curve's power and toe slope: `[1, 1]` for linear output.
    pub gamma: [f64; 2],
    /// The brightness scale.
    pub bright: f32,
    /// Whether LibRaw brightens the image by its histogram.
    pub auto_bright: bool,
    /// The white balance multipliers, used where neither `camera_wb` nor `auto_wb` is set.
    pub user_mul: [f32; 4],
    /// Whether LibRaw balances by the image's own average.
    pub auto_wb: bool,
    /// Whether LibRaw balances by the camera's as-shot multipliers.
    pub camera_wb: bool,
    /// The output's bits per sample: 8 or 16.
    pub output_bps: i32,
    /// The output color space: `0` for the sensor's own colors, `1` for sRGB.
    pub output_color: i32,
    /// The fraction of `maximum` within which LibRaw rescales by the frame's own brightest pixel;
    /// `0` never does.
    pub adjust_maximum_thr: f32,
    /// The orientation LibRaw turns the image by: `0` for the sensor's own rows, `-1` for the
    /// file's.
    pub user_flip: i32,
    /// Whether LibRaw turns a Fuji image by 45° and stretches it by its pixel aspect.
    pub fuji_rotate: bool,
    /// The demosaic: `None` for LibRaw's default.
    pub quality: Option<i32>,
}
