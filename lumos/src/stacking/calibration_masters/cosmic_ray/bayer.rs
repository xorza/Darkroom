//! Cosmic-ray rejection on a Bayer mosaic, by way of the mono detector.
//!
//! A Bayer mosaic is 2×2-periodic, so every pixel sharing a `(x % 2, y % 2)` phase is behind the
//! same filter and the four phases form dense same-colour planes. Deinterleaving them turns the
//! problem back into four mono detections, whose dense neighbours really are same-colour in the
//! mosaic. Pattern-independent: phase alone fixes the colour, so no `CfaPattern` is needed.

use rayon::prelude::*;

use crate::math::size2us::Size2us;
use crate::stacking::calibration_masters::cosmic_ray::config::CosmicRayConfig;
use crate::stacking::calibration_masters::cosmic_ray::mono::MonoDetector;

/// The Bayer detector: a mono detector, plus the buffer each phase is deinterleaved into.
///
/// Both are reused across all four phases. `(0, 0)` is the largest phase and runs first, so no
/// later one grows either allocation.
#[derive(Debug)]
pub(super) struct BayerDetector<'a> {
    mono: MonoDetector<'a>,
    plane: Vec<f32>,
}

impl<'a> BayerDetector<'a> {
    pub(super) fn new(config: &'a CosmicRayConfig) -> Self {
        Self {
            mono: MonoDetector::new(config),
            plane: Vec::new(),
        }
    }

    /// Clean every phase in place, returning the total CR pixel count across the four.
    ///
    /// Deinterleave and re-interleave are row-parallel like the detection between them. They are
    /// only a few percent of a frame today, but they are the whole of its *serial* fraction — the
    /// one part that would not shrink as thread count rises.
    pub(super) fn reject(&mut self, data: &mut [f32], size: Size2us) -> usize {
        let (w, h) = (size.width, size.height);
        let Self { mono, plane } = self;
        let mut total = 0;
        for b in 0..2 {
            for a in 0..2 {
                let pw = if a == 0 { w.div_ceil(2) } else { w / 2 };
                let ph = if b == 0 { h.div_ceil(2) } else { h / 2 };
                if pw < 3 || ph < 3 {
                    continue;
                }

                // Deinterleave: plane row j is mosaic row 2j+b, every second pixel from column a. Every
                // element is written, so `resize` only has to get the length right.
                plane.resize(pw * ph, 0.0);
                let mosaic = &*data;
                plane.par_chunks_mut(pw).enumerate().for_each(|(j, row)| {
                    let src = &mosaic[(j * 2 + b) * w..][..w];
                    for (i, o) in row.iter_mut().enumerate() {
                        *o = src[i * 2 + a];
                    }
                });

                total += mono.reject(plane, Size2us::new(pw, ph));

                // Re-interleave the cleaned plane. Chunking the mosaic by row keeps each thread's
                // writes to one row, so the phase's rows can be picked out of the full sweep.
                let cleaned = &plane[..];
                data.par_chunks_mut(w)
                    .enumerate()
                    .filter(|(y, _)| y % 2 == b)
                    .for_each(|(y, row)| {
                        for (i, &v) in cleaned[(y / 2) * pw..][..pw].iter().enumerate() {
                            row[i * 2 + a] = v;
                        }
                    });
            }
        }
        total
    }
}
