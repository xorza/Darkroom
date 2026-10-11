//! The 1-D kernels a warp resamples through.
//!
//! [`WarpKernel`](warp_kernel::WarpKernel) turns a filter into one output pixel's tap weights, at
//! the frame's stretch; the Lanczos table in `math::lanczos` and the Catmull-Rom polynomial here
//! are what it reads.

pub(super) mod warp_kernel;

#[cfg(test)]
pub(crate) mod internals {
    /// Catmull-Rom (`A = −½`), the definition the vector weights are held to.
    #[inline]
    pub(crate) fn bicubic_kernel(x: f32) -> f32 {
        const A: f32 = -0.5;
        let abs_x = x.abs();
        if abs_x <= 1.0 {
            ((A + 2.0) * abs_x - (A + 3.0)) * abs_x * abs_x + 1.0
        } else if abs_x < 2.0 {
            ((A * abs_x - 5.0 * A) * abs_x + 8.0 * A) * abs_x - 4.0 * A
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests;
