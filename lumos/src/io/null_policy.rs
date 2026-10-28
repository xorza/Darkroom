//! [`NullPolicy`]: what a sample with no measurement does to a linear decode, and the stand-in a
//! decoder writes under one.

use serde::Serialize;

use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::QualityFlags;
use crate::math::statistics;
use crate::math::statistics::subsample::Subsample;
use common::USize2;

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
    /// that mark [`QualityFlags::NO_DATA`] where a plane is null and [`QualityFlags::SATURATED`]
    /// where a channel reaches `saturation`, a level in the decoded domain, with every null filled
    /// ([`fill`]); `None` when they hold neither.
    ///
    /// A frame with no nulls and no saturation level returns before any scan of its pixels.
    pub(crate) fn resolve(
        self,
        planes: &mut [CountedPlane<'_>],
        size: USize2,
        saturation: Option<f32>,
    ) -> Result<Option<PixelFlags>, NullSamples> {
        let pixel_count = size.pixel_count();
        debug_assert!(
            planes
                .iter()
                .all(|plane| plane.samples.len() == pixel_count),
            "every plane holds {pixel_count} samples"
        );
        let count: usize = planes
            .iter()
            .filter_map(|plane| plane.nulls)
            .map(|nulls| nulls.count)
            .sum();
        if count == 0 && saturation.is_none() {
            return Ok(None);
        }
        if count > 0 && self == Self::Reject {
            // Samples, not pixels: each channel's nulls count apart, and the index is in the
            // channel-major sample space the count belongs to.
            let first_idx = planes
                .iter()
                .enumerate()
                .find_map(|(channel, plane)| {
                    plane
                        .nulls
                        .map(|nulls| channel * pixel_count + nulls.first_idx)
                })
                .expect("a nonzero count means at least one plane holds a null");
            return Err(NullSamples { count, first_idx });
        }
        // Before the fill, which is what erases the evidence. A non-finite sample fails every
        // comparison, so a null is never also saturated.
        let flags = PixelFlags::from_fn(size, |idx| {
            if planes.iter().any(|plane| !plane.samples[idx].is_finite()) {
                QualityFlags::NO_DATA
            } else if saturation
                .is_some_and(|level| planes.iter().any(|plane| plane.samples[idx] >= level))
            {
                QualityFlags::SATURATED
            } else {
                QualityFlags::default()
            }
        });
        for plane in planes.iter_mut() {
            if let Some(nulls) = plane.nulls {
                fill(plane.samples, nulls.count);
            }
        }
        Ok(flags)
    }
}

/// One decoded plane and its null samples, counted.
#[derive(Debug)]
pub(crate) struct CountedPlane<'a> {
    pub(crate) samples: &'a mut [f32],
    pub(crate) nulls: Option<PlaneNulls>,
}

impl<'a> CountedPlane<'a> {
    /// `samples`, their nulls counted here: for a decoder that did not count them as it decoded.
    pub(crate) fn count(samples: &'a mut [f32]) -> Self {
        let nulls = PlaneNulls::of(samples, 0);
        Self { samples, nulls }
    }
}

/// How many nulls a span of samples holds and where the first one is, in that span's own index
/// space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlaneNulls {
    pub(crate) count: usize,
    pub(crate) first_idx: usize,
}

impl PlaneNulls {
    /// The nulls of `samples`, which start at `start` of the index space; `None` without one.
    pub(crate) fn of(samples: &[f32], start: usize) -> Option<Self> {
        let mut count = 0;
        let mut first_idx = None;
        for (idx, sample) in samples.iter().enumerate() {
            if !sample.is_finite() {
                count += 1;
                first_idx.get_or_insert(start + idx);
            }
        }
        first_idx.map(|first_idx| Self { count, first_idx })
    }

    /// These nulls restated in an index space `by` samples earlier — a chunk's, in its plane's.
    pub(crate) const fn offset_by(self, by: usize) -> Self {
        Self {
            count: self.count,
            first_idx: self.first_idx + by,
        }
    }

    /// Fold in another span's nulls. Both must already be in the same index space.
    pub(crate) fn merge(self, other: Self) -> Self {
        Self {
            count: self.count + other.count,
            first_idx: self.first_idx.min(other.first_idx),
        }
    }
}

/// Null or non-finite samples of an image, under
/// [`NullPolicy::Reject`]: `count` of them, each channel's counted
/// apart, the first at `first_idx` of the channel-major sample space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{count} null/non-finite samples, first at linear index {first_idx}")]
pub struct NullSamples {
    pub count: usize,
    pub first_idx: usize,
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
    let mut measured = Subsample::data_values(
        samples.len(),
        missing_count,
        FILL_MEDIAN_SAMPLES,
        |idx| missing(idx, samples[idx]),
        |idx| samples[idx],
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
    use crate::image::flags::pixel_flags::PixelFlags;
    use crate::image::flags::quality_flags::QualityFlags;
    use crate::io::null_policy::{
        self, CountedPlane, FILL_MEDIAN_SAMPLES, NullPolicy, NullSamples,
    };
    use common::USize2;

    /// Two 2×1 planes with one null each, at different pixels. `Mask` flags both pixels and fills
    /// each plane with its own finite sample (the median of one value); `Reject` counts 2 samples,
    /// the first at index 1 of the channel-major space; planes with none give no flags, unless a
    /// saturation level of 0.6 flags the 0.75 at pixel 1 and not the 0.5 at pixel 0.
    #[test]
    fn a_policy_masks_or_rejects_the_nulls_of_every_plane() {
        let size = USize2::new(2, 1);
        let (mut red, mut green) = ([0.5, f32::NAN], [f32::INFINITY, 0.25]);
        let flags = NullPolicy::Mask
            .resolve(
                &mut [
                    CountedPlane::count(&mut red),
                    CountedPlane::count(&mut green),
                ],
                size,
                None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(flags.at(0), QualityFlags::NO_DATA);
        assert_eq!(flags.at(1), QualityFlags::NO_DATA);
        assert_eq!((red, green), ([0.5, 0.5], [0.25, 0.25]));

        let (mut red, mut green) = ([0.5, f32::NAN], [f32::INFINITY, 0.25]);
        let refused = NullPolicy::Reject
            .resolve(
                &mut [
                    CountedPlane::count(&mut red),
                    CountedPlane::count(&mut green),
                ],
                size,
                None,
            )
            .unwrap_err();
        assert_eq!(
            refused,
            NullSamples {
                count: 2,
                first_idx: 1
            }
        );

        let mut finite = [0.5, 0.75];
        let flags = NullPolicy::Reject.resolve(&mut [CountedPlane::count(&mut finite)], size, None);
        assert!(flags.unwrap().is_none());
        let saturated = NullPolicy::Reject
            .resolve(&mut [CountedPlane::count(&mut finite)], size, Some(0.6))
            .unwrap()
            .unwrap();
        assert_eq!(
            [saturated.at(0), saturated.at(1)],
            [QualityFlags::default(), QualityFlags::SATURATED]
        );
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
        let flags = PixelFlags::from_fn(USize2::new(6, 1), |idx| {
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
