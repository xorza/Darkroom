//! [`RunReport`]: what a run decided without failing, so that no decision is silent.

use crate::image::dark_comparison::DarkComparison;
use crate::image::flag_counts::FlagCounts;

/// The decisions a run took on its own and did not fail over, returned with its result.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RunReport {
    /// Channel samples the combine left out because their pixel carried the flag, with enough
    /// unflagged samples left at that pixel to keep the minimum survivor count. Only the flags the
    /// combine leaves samples out for count: saturated, defect, cosmic ray, repaired and flat floor.
    pub excluded_samples: FlagCounts,
    /// Channel samples the combine kept although their pixel carried the flag, because leaving
    /// them out would have dropped the pixel below the minimum survivor count. The same flags as
    /// [`Self::excluded_samples`] count.
    pub kept_flagged_samples: FlagCounts,
    /// The inverse variance plane holds the background noise only: some frame did not state its
    /// gain, so the photon noise of the signal above the sky is missing from the variance it
    /// inverts.
    pub variance_bg_only: bool,
    /// Lights calibrated with a dark whose exposure, or their own, was not declared, so the two
    /// were not compared.
    pub unverified_dark_exposures: u64,
    /// Lights calibrated with a dark with which they shared no temperature reading, so the two were
    /// not compared.
    pub unverified_dark_temperatures: u64,
    /// Lights whose dark was matched on the camera body's temperature, the two not both stating the
    /// sensor's: the weaker check, a body reading warmer than its sensor and lagging it.
    pub camera_temperature_darks: u64,
    /// Lights calibrated with a dark of another temperature, past the tolerance: warned, not
    /// refused. Each outcome holds the two temperatures.
    pub dark_temperature_differences: u64,
    /// Lights whose bias-removed dark was scaled by their exposure over its own, the fit having
    /// too few hot pixels, and the two exposures differing.
    pub scaled_darks: u64,
    /// Lights whose bias-removed dark was scaled by a fit on the hot pixels.
    pub fitted_dark_scales: u64,
    /// What the flat-dark was compared to the flat on, or the union over every frame the flat was
    /// stacked from; `None` without a flat-dark.
    pub flat_dark: Option<DarkComparison>,
    /// Photosites the flat's floor raised, corrected by less than their vignetting asks in every
    /// light.
    pub floored_flat_pixels: u64,
    /// Frames the combine read from disk because the set did not fit in memory: zero for a run
    /// that kept every frame resident.
    pub spilled_frames: u64,
    /// Channel pixels combined by the small-sample fallback: too few frames reached them for the
    /// rejection asked for. A median fallback leaves their inverse variance and dispersion
    /// unmeasured.
    pub small_n_fallback_pixels: u64,
    /// Bytes the run held past the memory budget, because not even the smallest step of one of
    /// its stages fit in it: one frame in work, the normalization's samples, or the combine's
    /// smallest chunk of rows beside its output planes. The stages run one after another, so this
    /// is the largest of their excesses; zero when every step fit.
    pub overcommit_bytes: u64,
    /// The heap the run's memory plan expected to hold at its peak: its largest stage at the width
    /// the plan chose, the bytes past the budget of a stage whose smallest step did not fit
    /// included. `None` for frames handed over with no plan.
    pub planned_peak_bytes: Option<u64>,
    /// Input pixels, over every frame of a drizzle, whose position a warp's SIP correction could
    /// not invert — see [`InverseWarp::apply`](crate::InverseWarp::apply). Each deposited nothing,
    /// so the output pixels it would have reached record that frame's absence in their coverage.
    /// Zero for frames registered without SIP, and for a stack that drizzled nothing.
    pub unconverged_points: u64,
    /// Calibrated lights written to disk between their detection and their registration, because
    /// the reference was not known until every light was detected: zero for a resident run, for
    /// one whose reference was named, which registers each light as it arrives, and for lights the
    /// decode cache keeps, which park where it keeps them.
    pub parked_lights: u64,
}
