//! [`RunReport`]: what a run decided without failing, so that no decision is silent.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::io::image::pixel_flags::QualityFlags;
use crate::io::image::unverified_conditions::UnverifiedConditions;

/// The decisions a run took on its own and did not fail over, returned with its result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunReport {
    /// Channel samples the combine left out because their pixel carried the flag, with enough
    /// unflagged samples left at that pixel to keep the minimum survivor count.
    pub excluded_samples: FlagCounts,
    /// Channel samples the combine kept although their pixel carried the flag, because leaving
    /// them out would have dropped the pixel below the minimum survivor count.
    pub kept_flagged_samples: FlagCounts,
    /// The variance plane holds the background noise only: some frame did not state its gain, so
    /// the photon noise of the signal above the sky is missing from it.
    pub variance_background_only: bool,
    /// Lights calibrated with a dark whose exposure, or their own, was not declared, so the two
    /// were not compared.
    pub unverified_dark_exposures: u64,
    /// Lights calibrated with a dark whose sensor temperature, or their own, was not declared.
    pub unverified_dark_temperatures: u64,
    /// Lights whose bias-removed dark was scaled to their exposure.
    pub scaled_darks: u64,
    /// The conditions not compared when the flat-dark was taken from the flat, or from any frame
    /// the flat was stacked from.
    pub unverified_flat_dark: UnverifiedConditions,
    /// Photosites the flat's floor raised, corrected by less than their vignetting asks in every
    /// light.
    pub floored_flat_pixels: u64,
    /// Frames the combine read from disk because the set did not fit in memory: zero for a run
    /// that kept every frame resident.
    pub spilled_frames: u64,
    /// Calibrated lights written to disk between their detection and their registration, because
    /// the reference was not known until every light was detected: zero for a resident run, and
    /// for one whose reference was named, which registers each light as it arrives.
    pub parked_lights: u64,
}

/// A count per data-quality flag a combine acts on. A sample with two flags counts under both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FlagCounts {
    pub saturated: u64,
    pub defect: u64,
    pub cosmic_ray: u64,
    pub repaired: u64,
    pub flat_floor: u64,
}

/// The flags a [`FlagCounts`] counts, in its field order.
const COUNTED: [QualityFlags; 5] = [
    QualityFlags::SATURATED,
    QualityFlags::DEFECT,
    QualityFlags::COSMIC_RAY,
    QualityFlags::REPAIRED,
    QualityFlags::FLAT_FLOOR,
];

/// One worker's [`FlagCounts`], in [`COUNTED`] order.
#[derive(Debug, Default)]
pub(crate) struct LocalFlagCounts([u64; 5]);

impl LocalFlagCounts {
    /// Count a sample carrying `flags` under each counted flag it holds.
    #[inline]
    pub(crate) fn count(&mut self, flags: QualityFlags) {
        for (count, flag) in self.0.iter_mut().zip(COUNTED) {
            *count += u64::from(flags.intersects(flag));
        }
    }
}

/// [`FlagCounts`] that parallel workers add into.
#[derive(Debug, Default)]
pub(crate) struct AtomicFlagCounts([AtomicU64; 5]);

impl AtomicFlagCounts {
    pub(crate) fn add(&self, local: &LocalFlagCounts) {
        for (total, &count) in self.0.iter().zip(&local.0) {
            if count > 0 {
                total.fetch_add(count, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn totals(&self) -> FlagCounts {
        let [saturated, defect, cosmic_ray, repaired, flat_floor] =
            self.0.each_ref().map(|count| count.load(Ordering::Relaxed));
        FlagCounts {
            saturated,
            defect,
            cosmic_ray,
            repaired,
            flat_floor,
        }
    }
}
