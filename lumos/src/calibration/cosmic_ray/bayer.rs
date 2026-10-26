//! Cosmic-ray rejection on a Bayer mosaic, by way of the mono detector.
//!
//! A Bayer mosaic is 2×2-periodic, so every pixel sharing a `(x % 2, y % 2)` phase is behind the
//! same filter and the four phases form dense same-color planes. Deinterleaving them turns the
//! problem back into four mono detections, whose dense neighbours really are same-color in the
//! mosaic. Pattern-independent: phase alone fixes the color, so no `BayerPattern` is needed.

use common::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::calibration::cosmic_ray::kept_out::KeptOut;
use crate::calibration::cosmic_ray::mono::MonoDetector;
use crate::calibration::cosmic_ray::noise_model::{NoiseModel, PixelBgs};
use crate::image::cfa::bayer_pattern::BayerPattern;
use crate::image::cfa::bayer_phase::BayerPhase;
use common::Size2us;

/// Phase-sized masks a phase's detection holds beside the mono detector's: the frame's [`KeptOut`]
/// sampled to the phase, two.
const PHASE_MASKS: usize = 2;

/// The Bayer detector: a mono detector, the buffers each phase is deinterleaved and kept out in,
/// and the mask of the photosites the four repaired.
///
/// The detector and the buffers are reused across all four phases. `(0, 0)` is the largest phase
/// and runs first, so no later one grows any allocation.
#[derive(Debug)]
pub(super) struct BayerDetector<'a> {
    mono: MonoDetector<'a>,
    pattern: BayerPattern,
    plane: Buffer2<f32>,
    /// The frame's [`KeptOut`] sampled to the phase at hand.
    plane_kept_out: KeptOut,
    /// The photosites the phases repaired, the mosaic's size.
    found: BitBuffer2,
}

impl<'a> BayerDetector<'a> {
    pub(super) fn new(
        config: &'a CosmicRayConfig,
        noise: NoiseModel,
        pattern: BayerPattern,
    ) -> Self {
        Self {
            mono: MonoDetector::new(config, noise),
            pattern,
            plane: Buffer2::default(),
            plane_kept_out: KeptOut::empty(),
            found: BitBuffer2::empty(),
        }
    }

    /// The bytes a detection on a `size` mosaic allocates beside it: the mask of what it repaired,
    /// and the largest phase plane, the mono detector's scratch over it and the phase's
    /// [`PHASE_MASKS`] — of which nothing when even that phase is too small to scan.
    pub(super) const fn heap_bytes(size: Size2us) -> usize {
        let [first, ..] = BayerPhase::ALL;
        let phase = first.size(size);
        let scanned = if MonoDetector::scans(phase) {
            phase.pixel_count() * size_of::<f32>()
                + MonoDetector::heap_bytes(phase)
                + PHASE_MASKS * BitBuffer2::heap_bytes(phase)
        } else {
            0
        };
        BitBuffer2::heap_bytes(size) + scanned
    }

    /// Clean every phase of `data` in place; the mask of every photosite it in-painted, the
    /// mosaic's size, which the detector holds until its next detection.
    pub(super) fn reject(
        &mut self,
        data: &mut Buffer2<f32>,
        bgs: &PixelBgs<'_>,
        kept_out: Option<&KeptOut>,
    ) -> &BitBuffer2 {
        let size = data.size();
        let Self {
            mono,
            pattern,
            plane,
            plane_kept_out,
            found,
        } = self;
        found.clear_to(size);
        for phase in BayerPhase::ALL {
            let plane_size = phase.size(size);
            if !MonoDetector::scans(plane_size) {
                continue;
            }
            phase.deinterleave(data, plane);
            let color = pattern.color_at(phase.origin()).idx();
            let local = |idx: usize| bgs.at(color, phase.to_mosaic(plane_size.point_of(idx)));
            let phase_kept_out = kept_out.map(|kept_out| {
                kept_out.sample_into(plane_kept_out, phase);
                &*plane_kept_out
            });
            mono.reject(plane, &local, phase_kept_out)
                .for_each_set(|pos| found.set_at(phase.to_mosaic(pos), true));
            phase.interleave(plane, data);
        }
        found
    }
}
