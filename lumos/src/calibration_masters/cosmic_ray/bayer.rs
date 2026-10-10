//! Cosmic-ray rejection on a Bayer mosaic, by way of the mono detector.
//!
//! A Bayer mosaic is 2×2-periodic, so every pixel sharing a `(x % 2, y % 2)` phase is behind the
//! same filter and the four phases form dense same-colour planes. Deinterleaving them turns the
//! problem back into four mono detections, whose dense neighbours really are same-colour in the
//! mosaic. Pattern-independent: phase alone fixes the colour, so no `CfaPattern` is needed.

use crate::bit_buffer2::BitBuffer2;
use crate::calibration_masters::cosmic_ray::config::CosmicRayConfig;
use crate::calibration_masters::cosmic_ray::kept_out::KeptOut;
use crate::calibration_masters::cosmic_ray::mono::MonoDetector;
use crate::calibration_masters::cosmic_ray::noise_model::{NoiseModel, PixelBackgrounds};
use crate::io::image::cfa::CfaType;
use crate::io::image::cfa::cfa_lattice::CfaLattice;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// The Bayer detector: a mono detector, the lattice that deinterleaves the phases, and the buffer
/// each phase is deinterleaved into.
///
/// The detector and the buffer are reused across all four phases. `(0, 0)` is the largest phase
/// and runs first, so no later one grows either allocation.
#[derive(Debug)]
pub(super) struct BayerDetector<'a> {
    mono: MonoDetector<'a>,
    lattice: CfaLattice,
    pattern: CfaPattern,
    plane: Vec<f32>,
}

impl<'a> BayerDetector<'a> {
    pub(super) fn new(config: &'a CosmicRayConfig, noise: NoiseModel, pattern: CfaPattern) -> Self {
        Self {
            mono: MonoDetector::new(config, noise),
            lattice: CfaLattice::new(&CfaType::Bayer(pattern)),
            pattern,
            plane: Vec::new(),
        }
    }

    /// The bytes a detection on a `size` mosaic allocates beside it: the largest phase plane and
    /// the mono detector's scratch over it, or nothing when even that phase is too small to scan.
    pub(super) fn heap_bytes(size: Size2us) -> usize {
        let phase = CfaLattice::phase_size(size, Vec2us::ZERO);
        match MonoDetector::heap_bytes(phase) {
            0 => 0,
            mono => phase.pixel_count() * size_of::<f32>() + mono,
        }
    }

    /// Clean every phase in place, marking every in-painted photosite in `found` (the mosaic's
    /// size) and returning the total across the four.
    pub(super) fn reject(
        &mut self,
        data: &mut [f32],
        size: Size2us,
        backgrounds: &PixelBackgrounds<'_>,
        kept_out: Option<&KeptOut>,
        found: &mut BitBuffer2,
    ) -> usize {
        let Self {
            mono,
            lattice,
            pattern,
            plane,
        } = self;
        let mut total = 0;
        for b in 0..2 {
            for a in 0..2 {
                let phase = Vec2us::new(a, b);
                let plane_size = CfaLattice::phase_size(size, phase);
                if plane_size.width < 3 || plane_size.height < 3 {
                    continue;
                }
                lattice.deinterleave(data, size, phase, plane);
                let colour = pattern.color_at(phase);
                let local = |index: usize| {
                    let at = plane_size.point_of(index);
                    backgrounds.at(colour, Vec2us::new(2 * at.x + a, 2 * at.y + b))
                };
                let plane_kept_out = kept_out.map(|kept_out| {
                    kept_out.sampled(plane_size, |at| Vec2us::new(2 * at.x + a, 2 * at.y + b))
                });
                let mut plane_found = BitBuffer2::new_default(plane_size);
                total += mono.reject(
                    plane,
                    plane_size,
                    &local,
                    plane_kept_out.as_ref(),
                    &mut plane_found,
                );
                plane_found.for_each_set(|pos| {
                    found.set_at(Vec2us::new(pos.x * 2 + a, pos.y * 2 + b), true);
                });
                lattice.interleave(plane, size, phase, data);
            }
        }
        total
    }
}
