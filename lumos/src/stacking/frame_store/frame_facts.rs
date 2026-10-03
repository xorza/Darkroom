//! [`FrameFacts`]: what a frame's decoder said its samples are.

use serde::{Deserialize, Serialize};

use crate::io::image::cfa::CfaType;
use crate::io::image::image_provenance::RowOrder;
use crate::io::image::sample_domain::SampleDomain;
use crate::stacking::frame_store::stackable_image::StackableImage;

/// What a frame's decoder said its samples are, which every frame of a set has to state alike —
/// see [`SetFacts`](crate::stacking::combine::cache::set_facts::SetFacts). Carried with the
/// frame's statistics because the metadata they come from is dropped for every frame but the
/// first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FrameFacts {
    /// What one sample is worth — see
    /// [`ImageMetadata::sample_domain`](crate::ImageMetadata::sample_domain).
    pub(crate) domain: Option<SampleDomain>,
    /// Which end of the image the first stored row belongs to — see [`RowOrder`].
    pub(crate) row_order: Option<RowOrder>,
    /// The mosaic pattern of an undemosaiced sensor frame.
    pub(crate) cfa_type: Option<CfaType>,
}

impl FrameFacts {
    pub(crate) fn of(image: &impl StackableImage) -> Self {
        Self {
            domain: image.metadata().sample_domain(),
            row_order: image.metadata().row_order(),
            cfa_type: image.cfa_type(),
        }
    }
}
