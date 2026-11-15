//! The HDR compression's two per-pixel steps as vector kernels: the log of the floored intensity,
//! and the factor that moves each pixel's residual toward the image's mean.
//!
//! Every lane, the partial last vector's too, takes its `ln` and `exp` by [`Math`], so a pixel's
//! value does not depend on where in the band it falls.

use simd::Math;
use simd::{F32_LANES, F32x8, Isa, Kernel};

use crate::image::intensity::IntensityBand;
use crate::image_ops::hdr::LOG_FLOOR;

/// `ln I` of each pixel's intensity `I` in a band, held to `[LOG_FLOOR, f32::MAX]` first, the
/// domain of [`Math::ln_f32`]: a NaN intensity takes the floor's log and an infinite one the
/// largest finite value's.
#[derive(Debug)]
pub(super) struct LogIntensity<'a> {
    pub(super) intensity: IntensityBand<'a>,
    pub(super) out: &'a mut [f32],
}

impl Kernel for LogIntensity<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let floor = isa.splat_f32(LOG_FLOOR);
        let ceiling = isa.splat_f32(f32::MAX);
        let (chunks, tail) = self.out.as_chunks_mut::<F32_LANES>();
        for (idx, out) in chunks.iter_mut().enumerate() {
            let intensity = load(isa, self.intensity, idx * F32_LANES);
            isa.ln_f32(intensity.max(floor).min(ceiling)).store(out);
        }
        if !tail.is_empty() {
            let intensity = load_partial(isa, self.intensity, chunks.len() * F32_LANES);
            isa.ln_f32(intensity.max(floor).min(ceiling))
                .store_partial(tail);
        }
    }
}

/// Each pixel's intensity times `exp(−amount·(r − mean))` of its residual `r`, in place of the
/// residual, over a band.
#[derive(Debug)]
pub(super) struct CompressFactor<'a> {
    pub(super) intensity: IntensityBand<'a>,
    pub(super) residual: &'a mut [f32],
    pub(super) mean: f32,
    pub(super) amount: f32,
}

impl Kernel for CompressFactor<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let mean = isa.splat_f32(self.mean);
        let minus_amount = isa.splat_f32(-self.amount);
        let (chunks, tail) = self.residual.as_chunks_mut::<F32_LANES>();
        for (idx, residual) in chunks.iter_mut().enumerate() {
            let intensity = load(isa, self.intensity, idx * F32_LANES);
            let r = isa.load_f32(residual);
            (intensity * isa.exp_f32(minus_amount * (r - mean))).store(residual);
        }
        if !tail.is_empty() {
            let intensity = load_partial(isa, self.intensity, chunks.len() * F32_LANES);
            let r = isa.load_f32_partial(tail);
            (intensity * isa.exp_f32(minus_amount * (r - mean))).store_partial(tail);
        }
    }
}

/// The intensities of the whole vector from `start`: the channel, or [`Rgb::intensity`] of the
/// three, op for op.
///
/// [`Rgb::intensity`]: crate::image::rgb::Rgb::intensity
#[inline(always)]
fn load<S: Isa>(isa: S, band: IntensityBand<'_>, start: usize) -> S::F32 {
    match band {
        IntensityBand::Mono(plane) => isa.load_f32(chunk(plane, start)),
        IntensityBand::Rgb([r, g, b]) => {
            (isa.load_f32(chunk(r, start))
                + isa.load_f32(chunk(g, start))
                + isa.load_f32(chunk(b, start)))
                * isa.splat_f32(1.0 / 3.0)
        }
    }
}

/// The whole vector of `plane` from `start`.
#[inline(always)]
fn chunk(plane: &[f32], start: usize) -> &[f32; F32_LANES] {
    plane[start..]
        .first_chunk()
        .expect("a whole vector from start")
}

/// [`load`] of the band's last partial vector, from `start` to its end, its missing lanes 0.
#[inline(always)]
fn load_partial<S: Isa>(isa: S, band: IntensityBand<'_>, start: usize) -> S::F32 {
    match band {
        IntensityBand::Mono(plane) => isa.load_f32_partial(&plane[start..]),
        IntensityBand::Rgb([r, g, b]) => {
            (isa.load_f32_partial(&r[start..])
                + isa.load_f32_partial(&g[start..])
                + isa.load_f32_partial(&b[start..]))
                * isa.splat_f32(1.0 / 3.0)
        }
    }
}

#[cfg(test)]
mod tests;
