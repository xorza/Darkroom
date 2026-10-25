//! [`NullPolicy`]: what a sample with no measurement does to a linear decode, and the stand-in a
//! decoder writes under one.

use serde::Serialize;

use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::math::statistics;
use crate::math::statistics::subsample::Subsample;
use common::Size2us;

/// What a linear image's null samples do to the load: a FITS null (IEEE NaN for a floating-point
/// `BITPIX`, the `BLANK` value for an integer one) or a floating-point raster's NaN or infinity.
///
/// A null is not corruption: FITS 4.0 §6.3 and the NOST floating-point agreement make IEEE NaN the
/// undefined-value flag for a floating-point `BITPIX`, and the `BLANK` keyword's value the flag for
/// an integer one. A conforming file with a mosaic edge, a coverage gap, or a masked bad pixel
/// carries them by design, and cfitsio, astropy and Siril all read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum NullPolicy {
    /// Carry them: the samples are replaced with a finite fill and flagged
    /// [`QualityFlags::NO_DATA`], so the file opens and the missing pixels stay identifiable.
    ///
    /// The default: reading a file that conforms to the standard is not opt-in behaviour. The other
    /// load defaults refuse *ambiguity* — a cube whose color semantics nothing establishes, a
    /// float scale no header settles; a null is unambiguous, and refusing it refuses data the
    /// standard defines.
    #[default]
    Mask,
    /// Fail the load, naming how many there are and where the first one is.
    Reject,
}

impl NullPolicy {
    /// This policy applied to the decoded `planes` of one image, each `size` pixels: the flags
    /// that mark their null pixels, with every null filled ([`fill`]), or `None` when they hold
    /// none.
    pub(crate) fn resolve(
        self,
        planes: &mut [&mut [f32]],
        size: Size2us,
    ) -> Result<Option<PixelFlags>, NullSamples> {
        let pixel_count = size.pixel_count();
        let mut count = 0;
        let mut first_idx = None;
        for (channel, plane) in planes.iter().enumerate() {
            #[expect(
                clippy::debug_assert_with_mut_call,
                reason = "`len` reads the `&mut [f32]` through a shared borrow"
            )]
            {
                debug_assert_eq!(plane.len(), pixel_count);
            }
            for (idx, sample) in plane.iter().enumerate() {
                if !sample.is_finite() {
                    count += 1;
                    first_idx.get_or_insert(channel * pixel_count + idx);
                }
            }
        }
        let Some(first_idx) = first_idx else {
            return Ok(None);
        };
        if self == Self::Reject {
            return Err(NullSamples { count, first_idx });
        }
        let flags = PixelFlags::from_fn(size, |idx| {
            if planes.iter().any(|plane| !plane[idx].is_finite()) {
                QualityFlags::NO_DATA
            } else {
                QualityFlags::default()
            }
        });
        for plane in planes.iter_mut() {
            let nulls = plane.iter().filter(|sample| !sample.is_finite()).count();
            if nulls > 0 {
                fill(plane, nulls);
            }
        }
        Ok(flags)
    }
}

/// Null or non-finite samples of an image, under
/// [`NullPolicy::Reject`]: `count` of them, each channel's counted
/// apart, the first at `first_idx` of the channel-major sample space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{count} null/non-finite samples, first at linear index {first_idx}")]
pub(crate) struct NullSamples {
    pub(crate) count: usize,
    pub(crate) first_idx: usize,
}

/// Samples the fill's median is taken over, at most.
///
/// The median of a hundred thousand values drawn evenly across a plane and the median of all
/// twenty-four million agree to far more precision than a stand-in for missing data needs, and the
/// bound is what keeps this off the memory preflight: the scratch is a fixed 400 KB rather than a
/// second copy of the plane the preflight did not budget for. `defect_map::sampling` bounds its own
/// medians the same way and for the same reason.
const FILL_MEDIAN_SAMPLES: usize = 100_000;

/// Replace a plane's `null_count` non-finite samples with the median of its finite ones.
///
/// What sits under a null is not data and [`QualityFlags::NO_DATA`] says so, but the stages that
/// measure a whole plane mostly do not consult the flags, so this value is what they see. The
/// median is the frame's own background level, which leaves a masked region a flat patch instead of
/// the hard-edged hole a zero fill would cut — and a hard edge is what manufactures star detections
/// and drags a background estimate. A deliberate stand-in, not a correction.
pub(crate) fn fill(samples: &mut [f32], null_count: usize) {
    fill_where(samples, |_, value| !value.is_finite(), null_count);
}

/// Replace the samples of a plane that `flags` mark [`QualityFlags::NO_DATA`] with the median of
/// the rest, as [`fill`] replaces a null: a decoder that reads a photosite as no measurement
/// leaves the same stand-in under it as one that reads a null.
pub(super) fn fill_flagged(samples: &mut [f32], flags: &PixelFlags) {
    let count = flags.count(QualityFlags::NO_DATA);
    if count > 0 {
        let missing = |idx: usize, _| flags.at(idx).intersects(QualityFlags::NO_DATA);
        fill_where(
            samples,
            missing,
            usize::try_from(count).expect("a plane's pixel count"),
        );
    }
}

/// Replace the `missing_count` samples `missing` names, by index and value, with the median of
/// the others.
fn fill_where(samples: &mut [f32], missing: impl Fn(usize, f32) -> bool, missing_count: usize) {
    debug_assert!(missing_count > 0 && missing_count <= samples.len());
    // Every `stride`-th *measured* sample rather than every `stride`-th sample: missing pixels
    // arrive in regions — a mosaic edge, a coverage gap — so striding the plane itself would draw
    // its whole quota from one side of a frame that is masked down the other.
    let sample = Subsample::new(samples.len() - missing_count, FILL_MEDIAN_SAMPLES);
    let mut measured = Vec::with_capacity(sample.count());
    measured.extend(
        samples
            .iter()
            .enumerate()
            .filter(|&(idx, &value)| !missing(idx, value))
            .map(|(_, &value)| value)
            .step_by(sample.stride()),
    );
    // A wholly-missing plane has no level of its own to borrow, and no guess is better than any
    // other. The flags say every pixel of it is missing, which is the part that has to survive.
    let fill = if measured.is_empty() {
        0.0
    } else {
        statistics::median_mut(&mut measured)
    };
    for (idx, value) in samples.iter_mut().enumerate() {
        if missing(idx, *value) {
            *value = fill;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::image::pixel_flags::PixelFlags;
    use crate::image::quality_flags::QualityFlags;
    use crate::io::null_policy::{self, FILL_MEDIAN_SAMPLES, NullPolicy, NullSamples};
    use common::Size2us;

    /// Two 2×1 planes with one null each, at different pixels. `Mask` flags both pixels and fills
    /// each plane with its own finite sample (the median of one value); `Reject` counts 2 samples,
    /// the first at index 1 of the channel-major space; planes with none give no flags.
    #[test]
    fn a_policy_masks_or_rejects_the_nulls_of_every_plane() {
        let size = Size2us::new(2, 1);
        let (mut red, mut green) = ([0.5, f32::NAN], [f32::INFINITY, 0.25]);
        let flags = NullPolicy::Mask
            .resolve(&mut [&mut red, &mut green], size)
            .unwrap()
            .unwrap();
        assert_eq!(flags.at(0), QualityFlags::NO_DATA);
        assert_eq!(flags.at(1), QualityFlags::NO_DATA);
        assert_eq!((red, green), ([0.5, 0.5], [0.25, 0.25]));

        let (mut red, mut green) = ([0.5, f32::NAN], [f32::INFINITY, 0.25]);
        let refused = NullPolicy::Reject
            .resolve(&mut [&mut red, &mut green], size)
            .unwrap_err();
        assert_eq!(
            refused,
            NullSamples {
                count: 2,
                first_idx: 1
            }
        );

        let mut finite = [0.5, 0.75];
        let flags = NullPolicy::Reject.resolve(&mut [&mut finite], size);
        assert!(flags.unwrap().is_none());
    }

    #[test]
    fn nulls_are_filled_with_the_median_of_the_finite_samples() {
        // Finite samples 1, 2, 3, 4, 10 — five of them, so the median is the middle one, 3. Both
        // nulls take it, and no finite sample moves.
        let mut samples = [1.0, f32::NAN, 2.0, 3.0, f32::INFINITY, 4.0, 10.0];
        null_policy::fill(&mut samples, 2);
        assert_eq!(samples, [1.0, 3.0, 2.0, 3.0, 3.0, 4.0, 10.0]);

        // Zero fills a plane with no finite sample to borrow a level from: nothing else is more
        // right, and the mask is what records that none of it is data.
        let mut empty = [f32::NAN; 3];
        null_policy::fill(&mut empty, 3);
        assert_eq!(empty, [0.0; 3]);

        // Flagged samples take the median of the unflagged ones whatever they hold, a finite value
        // included: the unflagged 1, 2, 4 and 10 have the median (2 + 4) / 2 = 3, and the flagged
        // 0.5 and 7 take it.
        let mut samples = [1.0, 0.5, 2.0, 4.0, 7.0, 10.0];
        let missing = [1, 4];
        let flags = PixelFlags::from_fn(Size2us::new(6, 1), |idx| {
            if missing.contains(&idx) {
                QualityFlags::NO_DATA
            } else {
                QualityFlags::default()
            }
        })
        .unwrap();
        null_policy::fill_flagged(&mut samples, &flags);
        assert_eq!(samples, [1.0, 3.0, 2.0, 4.0, 3.0, 10.0]);
    }

    #[test]
    fn the_fills_median_is_sampled_rather_than_taken_over_a_whole_plane() {
        // Twice the sample bound of valid data, so the stride is exactly 2 and the scratch holds
        // `FILL_MEDIAN_SAMPLES` values instead of a second copy of the plane — the allocation the
        // decode's memory preflight does not budget for.
        //
        // Valid samples are the ramp 0..200_000. Every second one is 0, 2, … 199_998, whose median
        // is the mean of its two middle values (99_998 and 100_000) = 99_999. The whole ramp's
        // median is 99_999.5, so the bound costs half a unit in two hundred thousand.
        const VALID: usize = 2 * FILL_MEDIAN_SAMPLES;
        let mut samples: Vec<f32> = (0..VALID).map(|idx| idx as f32).collect();
        samples.push(f32::NAN);
        null_policy::fill(&mut samples, 1);
        assert_eq!(samples[VALID], 99_999.0);

        // The stride runs over the finite samples, not over plane positions: masking the whole
        // first half would otherwise spend that half of the quota on pixels that are dropped
        // anyway, and draw the median from the tail alone.
        let mut lopsided: Vec<f32> = (0..VALID)
            .map(|idx| {
                if idx < VALID / 2 {
                    f32::NAN
                } else {
                    idx as f32
                }
            })
            .collect();
        null_policy::fill(&mut lopsided, VALID / 2);
        // The surviving ramp is 100_000..200_000, whose median is 149_999.5; sampling every one of
        // them (stride 1, since the survivors are exactly the bound) reproduces it exactly.
        assert_eq!(lopsided[0], 149_999.5);
    }
}
