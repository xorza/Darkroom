//! [`KeptOut`]: the pixels a cosmic-ray pass flags none of, and those it in-paints from none of.

use common::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::image::cfa::bayer_phase::BayerPhase;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::QualityFlags;
use crate::math::statistics;
use crate::math::statistics::radix_median::RadixMedian;
use glam::USizeVec2;

/// The 5×5 window, as astroscrappy's `medfilt5`, whose median tells a saturated star's core from a
/// saturated hit.
const CORE_RADIUS: usize = 2;
/// The grown star mask's reach on each axis: astroscrappy's `dilate5(satpixels, 2)`, two dilations
/// by the 5×5 kernel less its corners. A step of the kernel moves at most 2 on each axis and not 2
/// on both, so two of them reach `max(|dx|, |dy|) ≤ 4` with `|dx| + |dy| ≤ 6`, an octagon.
const STAR_REACH: isize = 4;
/// The octagon's bound on `|dx| + |dy|`.
const STAR_TAXICAB: isize = 6;
/// A saturated pixel is a star's core when its 5×5 median passes this share of the saturation
/// level: astroscrappy's `m5 > satlevel / 10`.
const CORE_SHARE: f32 = 0.1;

/// The pixels one cosmic-ray pass keeps out, as astroscrappy's `update_mask` does with its
/// saturated stars and bad pixels.
///
/// A saturated star's core is a flat top with steep edges, L.A.Cosmic's classic false positive: a
/// saturated pixel whose 5×5 median passes a tenth of the saturation level is one, and the mask
/// grows 4 pixels around it. A hit that saturates a single pixel has a low median and stays a
/// candidate. The frame's saturation level is the median of its saturated samples: calibration
/// moved the units, and they sit at it.
#[derive(Debug)]
pub(crate) struct KeptOut {
    /// The star cores, grown, and the pixels with no measurement: never a candidate, never grown
    /// into, and left out of the background.
    pub(crate) never_flagged: BitBuffer2,
    /// [`Self::never_flagged`] and every saturated pixel: no in-paint reads them, as a bound or a
    /// fill is not a neighbour's value.
    pub(crate) unread: BitBuffer2,
}

impl KeptOut {
    /// What `flags` keep out of a pass over `data`; `None` when it keeps out nothing.
    pub(crate) fn of(data: &Buffer2<f32>, flags: Option<&PixelFlags>) -> Option<Self> {
        let flags = flags.filter(|flags| flags.contains(QualityFlags::UNMEASURED))?;
        let saturated = flags.mask_of(QualityFlags::SATURATED);
        let mut never_flagged = grown(&star_cores(data, &saturated));
        never_flagged.or_with(&flags.mask_of(QualityFlags::NO_DATA));
        let mut unread = saturated;
        unread.or_with(&never_flagged);
        Some(Self {
            never_flagged,
            unread,
        })
    }

    /// Nothing kept out, of no pixels: what [`Self::sample_into`] fills.
    pub(crate) const fn empty() -> Self {
        Self {
            never_flagged: BitBuffer2::empty(),
            unread: BitBuffer2::empty(),
        }
    }

    /// Into `plane`, the same over the photosites of `phase`. Fills `plane`'s allocations, which
    /// grow only for a larger plane.
    pub(super) fn sample_into(&self, plane: &mut Self, phase: BayerPhase) {
        phase.deinterleave_mask(&self.never_flagged, &mut plane.never_flagged);
        phase.deinterleave_mask(&self.unread, &mut plane.unread);
    }
}

/// `cores` grown by the octagon of [`STAR_REACH`] and [`STAR_TAXICAB`], clipped to the frame.
fn grown(cores: &BitBuffer2) -> BitBuffer2 {
    let size = cores.size();
    let mut mask = BitBuffer2::new_default(size);
    cores.for_each_set(|core| {
        for dy in -STAR_REACH..=STAR_REACH {
            for dx in -STAR_REACH..=STAR_REACH {
                if dx.abs() + dy.abs() > STAR_TAXICAB {
                    continue;
                }
                if let (Some(x), Some(y)) = (
                    core.x.checked_add_signed(dx).filter(|&x| x < size.width),
                    core.y.checked_add_signed(dy).filter(|&y| y < size.height),
                ) {
                    mask.set_at(USizeVec2::new(x, y), true);
                }
            }
        }
    });
    mask
}

/// The saturated pixels of `data` whose 5×5 median, the border replicated, passes a tenth of the
/// saturation level, the median of the saturated samples.
fn star_cores(data: &Buffer2<f32>, saturated: &BitBuffer2) -> BitBuffer2 {
    let size = saturated.size();
    let mut cores = BitBuffer2::new_default(size);
    // Ranked in place rather than gathered: a frame can be saturated anywhere, and a copy of its
    // saturated samples would be a plane no budget charges.
    let mut level = RadixMedian::default();
    let mut high = level.high_pass();
    saturated.for_each_set(|pos| high.add(data[size.idx_of(pos)]));
    if high.len() == 0 {
        return cores;
    }
    let mut low = high.finish();
    saturated.for_each_set(|pos| low.add(data[size.idx_of(pos)]));
    let threshold = CORE_SHARE * low.median();
    let mut window = Vec::with_capacity((2 * CORE_RADIUS + 1).pow(2));
    saturated.for_each_set(|pos| {
        window.clear();
        for dy in 0..=2 * CORE_RADIUS {
            let y = (pos.y + dy)
                .saturating_sub(CORE_RADIUS)
                .min(size.height - 1);
            for dx in 0..=2 * CORE_RADIUS {
                let x = (pos.x + dx).saturating_sub(CORE_RADIUS).min(size.width - 1);
                window.push(data[size.idx_of(USizeVec2::new(x, y))]);
            }
        }
        if statistics::median_mut(&mut window) > threshold {
            cores.set_at(pos, true);
        }
    });
    cores
}
