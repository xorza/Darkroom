//! The Lanczos row warp as a vector kernel, dispatched once per output row.
//!
//! A window's row is one vector: [`F32_LANES`] lanes hold Lanczos4's eight taps, and Lanczos2/3's
//! four or six with the lanes past them zero-padded, so a window reads exactly its own pixels.
//! Every Isa folds the products in one order, so a warped pixel is the same on every CPU.

use std::array;

use crate::math::size2us::Size2us;
use crate::registration::resample::kernel;
use crate::registration::resample::kernel::{LANCZOS_LUT_RESOLUTION, LanczosLut, LanczosOrder};
use crate::registration::resample::source_position::{SourcePosition, window_inside};
use crate::simd::{F32_LANES, F32x8, Isa, Kernel};
use imaginarium::Buffer2;

/// Lanczos-`A` over each `SIZE = 2A` window, on the widest Isa this CPU has: the vector kernel
/// where the window lies inside the source, and edge-extended bilinear where it straddles the
/// border — truncating a signed kernel can leave arbitrarily little weight, which bilinear's
/// clamped taps cannot.
///
/// An interior window is divided by its tap-weight total, which for a full Lanczos window lies
/// within a few per cent of 1 at every fraction, so it needs no guard.
pub(super) fn lanczos_row<const A: usize, const SIZE: usize>(
    order: LanczosOrder,
    input: &Buffer2<f32>,
    positions: &[Option<SourcePosition>],
    border_value: f32,
    output_row: &mut [f32],
) {
    debug_assert_eq!(order.a(), A);
    LanczosRow::<A, SIZE> {
        lut: order.lut(),
        input,
        positions,
        border_value,
        output_row,
    }
    .dispatch();
}

/// [`lanczos_row`] as a kernel.
#[derive(Debug)]
struct LanczosRow<'a, const A: usize, const SIZE: usize> {
    lut: &'a LanczosLut,
    input: &'a Buffer2<f32>,
    positions: &'a [Option<SourcePosition>],
    border_value: f32,
    output_row: &'a mut [f32],
}

impl<const A: usize, const SIZE: usize> Kernel for LanczosRow<'_, A, SIZE> {
    type Output = ();

    #[inline(always)]
    #[expect(
        clippy::cast_sign_loss,
        reason = "a window `window_inside` accepted starts at a non-negative tap"
    )]
    fn run<S: Isa>(self, isa: S) {
        const { assert!(SIZE == 2 * A && SIZE <= F32_LANES) };
        let pixels = self.input.pixels();
        let width = self.input.width();
        let size = Size2us::new(width, self.input.height());
        let reach = A as i32 - 1;

        for (value, position) in self.output_row.iter_mut().zip(self.positions) {
            let Some(position) = *position else {
                *value = self.border_value;
                continue;
            };
            let origin = position.window_origin(reach);
            if !window_inside(origin, SIZE, size) {
                *value = kernel::bilinear_sample(self.input, position);
                continue;
            }
            let (kx, ky) = (origin.x as usize, origin.y as usize);

            let wx = tap_weights::<S, A, SIZE>(isa, self.lut, position.fx);
            let wy = tap_weights::<S, A, SIZE>(isa, self.lut, position.fy);
            let inv_total = 1.0 / (wx.iter().sum::<f32>() * wy.iter().sum::<f32>());

            let wx_lanes = isa.load_f32_partial(&wx);
            let mut sum = isa.splat_f32(0.0);
            for (j, &wyj) in wy.iter().enumerate() {
                let row = &pixels[(ky + j) * width + kx..][..SIZE];
                sum = (isa.load_f32_partial(row) * wx_lanes).mul_add(isa.splat_f32(wyj), sum);
            }
            *value = sum.reduce_sum() * inv_total;
        }
    }
}

/// The `SIZE` separable tap weights for fractional offset `frac`, bit for bit
/// [`LanczosLut::weights`]: one vector of distances and one table lookup per lane, rather than
/// `SIZE` scalar lookups.
///
/// The lookup only pays off once six or more taps share it, so Lanczos2 takes the scalar weights —
/// the gathered form measured ~6% slower there on AVX2. `SIZE` is const, so the branch folds away.
#[inline(always)]
fn tap_weights<S: Isa, const A: usize, const SIZE: usize>(
    isa: S,
    lut: &LanczosLut,
    frac: f32,
) -> [f32; SIZE] {
    if SIZE <= 4 {
        return lut.weights::<SIZE>(frac);
    }
    let tables = const { DistanceTables::new::<A, SIZE>() };
    // `base ± frac` and `distance · RES + 0.5`, unfused, round exactly as the scalar lookup's
    // `(a − 1 − i) + frac`, `(i + 1 − a) − frac` and `x · RES + 0.5` do; the lookup then
    // truncates as its `as usize` does.
    let distance = isa.load_f32(&tables.base) + isa.load_f32(&tables.sign) * isa.splat_f32(frac);
    let index = distance * isa.splat_f32(LANCZOS_LUT_RESOLUTION as f32) + isa.splat_f32(0.5);
    let lanes = isa.lookup_f32(&lut.values, index).to_array();
    array::from_fn(|i| lanes[i])
}

/// The per-tap distance affine `dist[i] = base[i] + sign[i]·frac` that [`tap_weights`] loads.
/// Determined by `A` and `SIZE` alone, so it is built in a `const` block rather than at run time;
/// lanes `SIZE..8` stay zero, looking up entry 0 as an in-bounds dummy.
#[derive(Debug)]
struct DistanceTables {
    base: [f32; F32_LANES],
    sign: [f32; F32_LANES],
}

impl DistanceTables {
    /// For `i < A` the tap sits left of centre and its distance grows with `frac`; for `i ≥ A` it
    /// sits right of centre and shrinks with it, hence the sign flip.
    const fn new<const A: usize, const SIZE: usize>() -> Self {
        let mut base = [0.0f32; F32_LANES];
        let mut sign = [0.0f32; F32_LANES];
        let a_minus_1 = A as i32 - 1;
        let mut i = 0;
        while i < SIZE {
            if i < A {
                base[i] = (a_minus_1 - i as i32) as f32;
                sign[i] = 1.0;
            } else {
                base[i] = (i as i32 - a_minus_1) as f32;
                sign[i] = -1.0;
            }
            i += 1;
        }
        Self { base, sign }
    }
}

#[cfg(test)]
mod tests;
