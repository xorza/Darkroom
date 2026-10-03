//! The Lanczos kernel on every tier: its tap weights against the table, and its rows against
//! `Portable`'s.

use crate::internals::prelude::*;
use crate::registration::resample::kernel::{LanczosLut, LanczosOrder};
use crate::registration::resample::row::simd::{LanczosRow, tap_weights};
use crate::registration::resample::row_positions::RowPositions;
use crate::registration::transform::{Transform, WarpTransform};
use crate::simd::tier::Tier;
use crate::simd::{Isa, Kernel};

/// [`tap_weights`] on whichever Isa a tier runs.
#[derive(Debug)]
struct Weights<'a, const A: usize, const SIZE: usize> {
    lut: &'a LanczosLut,
    frac: f32,
}

impl<const A: usize, const SIZE: usize> Kernel for Weights<'_, A, SIZE> {
    type Output = [f32; SIZE];

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) -> [f32; SIZE] {
        tap_weights::<S, A, SIZE>(isa, self.lut, self.frac)
    }
}

/// The looked-up tap weights equal the table's scalar `weights` exactly, on every tier, at every
/// 1/1024 of a pixel and at the fraction just below 1: both index the same table by the same
/// rounded distance.
#[test]
fn tap_weights_match_the_scalar_lookups() {
    fn check<const A: usize, const SIZE: usize>(order: LanczosOrder) {
        let lut = order.lut();
        let fracs = (0..1024)
            .map(|k| k as f32 / 1024.0)
            .chain([1.0 - f32::EPSILON / 2.0, 1.0]);
        for tier in Tier::supported() {
            for frac in fracs.clone() {
                assert_eq!(
                    tier.run(Weights::<A, SIZE> { lut, frac }),
                    lut.weights::<SIZE>(frac),
                    "{tier} Lanczos{A} at {frac}"
                );
            }
        }
    }
    check::<2, 4>(LanczosOrder::Two);
    check::<3, 6>(LanczosOrder::Three);
    check::<4, 8>(LanczosOrder::Four);
}

/// One output row warped by `order` on `tier`.
fn warp_row(
    tier: Tier,
    order: LanczosOrder,
    input: &Buffer2<f32>,
    positions: &RowPositions,
) -> Vec<f32> {
    let mut output_row = vec![f32::NAN; input.width()];
    let lut = order.lut();
    let positions = positions.positions();
    match order {
        LanczosOrder::Two => tier.run(LanczosRow::<2, 4> {
            lut,
            input,
            positions,
            border_value: 0.0,
            output_row: &mut output_row,
        }),
        LanczosOrder::Three => tier.run(LanczosRow::<3, 6> {
            lut,
            input,
            positions,
            border_value: 0.0,
            output_row: &mut output_row,
        }),
        LanczosOrder::Four => tier.run(LanczosRow::<4, 8> {
            lut,
            input,
            positions,
            border_value: 0.0,
            output_row: &mut output_row,
        }),
    }
    output_row
}

/// Every tier warps a row to `Portable`'s bits, on rows that cross the border, the interior and
/// the right edge, where a window's last tap is the image's last column. The oracle in the row
/// tests holds the dispatched row to the definition; this holds every tier to that row.
#[test]
fn every_tier_warps_to_portables_bits() {
    let size = Size2us::new(41, 33);
    let input = Buffer2::new(
        size.width,
        size.height,
        (0..size.pixel_count())
            .map(|i| ((i * 13 + i / size.width * 7) % 31) as f32 / 9.0 - 1.7)
            .collect(),
    );
    let transform = WarpTransform::new(Transform::similarity(DVec2::new(1.5, -0.75), 0.07, 1.03));
    let mut positions = RowPositions::default();
    for order in [LanczosOrder::Two, LanczosOrder::Three, LanczosOrder::Four] {
        for y in [0, size.height / 2, size.height - 1] {
            positions.fill(y, size.width, &transform, size);
            let reference = warp_row(Tier::portable(), order, &input, &positions);
            for tier in Tier::supported() {
                let row = warp_row(tier, order, &input, &positions);
                assert_eq!(
                    row.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    reference.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    "{tier} {order:?} row {y}"
                );
            }
        }
    }
}
