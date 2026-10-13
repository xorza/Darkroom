//! [`FrameFacts`]: what a frame's decoder said its samples are.

use serde::{Deserialize, Serialize};

use crate::frame::capture_conditions::CaptureConditions;
use crate::frame::stackable_image::StackableImage;
use crate::frame::unverified_conditions::UnverifiedConditions;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::image_provenance::RowOrder;
use crate::image::sample_domain::SampleDomain;

/// What a frame's source said about it. The domain, row order and pattern every frame of a set has
/// to state alike — see `ingest::set_facts::SetFacts`; the capture
/// conditions the set's product states only where every frame shares them, and the conditions a
/// dark match left uncompared wherever any frame did. Carried with the frame's
/// statistics because the metadata they come from is dropped for every frame but the first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FrameFacts {
    /// What one sample is worth — see [`ImageMetadata::domain`](crate::ImageMetadata::domain).
    pub(crate) domain: Option<SampleDomain>,
    /// Which end of the image the first stored row belongs to — see [`RowOrder`].
    pub(crate) row_order: Option<RowOrder>,
    /// The mosaic pattern of an undemosaiced sensor frame.
    pub(crate) cfa_type: Option<CfaType>,
    /// Whether the frame's decoder flagged every saturated pixel — see
    /// [`ImageMetadata::saturation_flagged`](crate::ImageMetadata::saturation_flagged).
    pub(crate) saturation_flagged: bool,
    /// The exposure and temperature the frame was taken under.
    pub(crate) conditions: CaptureConditions,
    /// The conditions not compared when a dark was taken from the frame — see
    /// [`ImageMetadata::unverified_dark`](crate::ImageMetadata::unverified_dark).
    pub(crate) unverified_dark: UnverifiedConditions,
}

impl FrameFacts {
    pub(crate) fn of(image: &impl StackableImage) -> Self {
        Self {
            domain: image.metadata().domain.clone(),
            row_order: image.metadata().row_order(),
            cfa_type: image.cfa_type(),
            saturation_flagged: image.metadata().saturation_flagged,
            conditions: CaptureConditions::of(image.metadata()),
            unverified_dark: image.metadata().unverified_dark,
        }
    }
}
