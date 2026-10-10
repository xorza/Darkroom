//! [`FrameFacts`]: what a frame's decoder said its samples are.

use serde::{Deserialize, Serialize};

use crate::frame_store::capture_conditions::CaptureConditions;
use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::cfa::CfaType;
use crate::io::image::image_provenance::RowOrder;
use crate::io::image::sample_domain::SampleDomain;

/// What a frame's source said about it. The domain, row order and pattern every frame of a set has
/// to state alike — see [`SetFacts`](crate::combine::cache::set_facts::SetFacts); the capture
/// conditions the set's product states only where every frame shares them. Carried with the frame's
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
}

impl FrameFacts {
    pub(crate) fn of(image: &impl StackableImage) -> Self {
        Self {
            domain: image.metadata().domain.clone(),
            row_order: image.metadata().row_order(),
            cfa_type: image.cfa_type(),
            saturation_flagged: image.metadata().saturation_flagged,
            conditions: CaptureConditions::of(image.metadata()),
        }
    }
}
