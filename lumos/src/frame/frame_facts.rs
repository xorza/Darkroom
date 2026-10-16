//! [`FrameFacts`]: what a frame's decoder said its samples are.

use serde::{Deserialize, Serialize};

use crate::frame::capture_conditions::CaptureConditions;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::dark_comparison::DarkComparison;
use crate::image::image_provenance::RowOrder;
use crate::image::sample_domain::SampleDomain;
use crate::image::stackable_image::StackableImage;

/// What a frame's source said about it. The domain, row order and pattern every frame of a set has
/// to state alike — see `ingest::set_facts::SetFacts`; the capture
/// conditions the set's product states only where every frame shares them, and the conditions a
/// dark match left uncompared wherever any frame did. Carried with the frame's
/// statistics because the metadata they come from is dropped for every frame but the first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FrameFacts {
    /// What one sample is worth — see [`DerivedFacts::domain`](crate::DerivedFacts::domain).
    pub(crate) domain: Option<SampleDomain>,
    /// Which end of the image the first stored row belongs to — see [`RowOrder`].
    pub(crate) row_order: Option<RowOrder>,
    /// The mosaic pattern of an undemosaiced sensor frame.
    pub(crate) cfa_type: Option<CfaType>,
    /// Whether the frame's decoder flagged every saturated pixel — see
    /// [`DerivedFacts::saturation_flagged`](crate::DerivedFacts::saturation_flagged).
    pub(crate) saturation_flagged: bool,
    /// The exposure and temperature the frame was taken under.
    pub(crate) conditions: CaptureConditions,
    /// What taking a dark from the frame compared — see
    /// [`DerivedFacts::dark_comparison`](crate::DerivedFacts::dark_comparison).
    pub(crate) dark_comparison: Option<DarkComparison>,
}

impl FrameFacts {
    pub(crate) fn of(image: &impl StackableImage) -> Self {
        Self {
            domain: image.metadata().derived.domain().cloned(),
            row_order: image.metadata().derived.row_order(),
            cfa_type: image.cfa_type(),
            saturation_flagged: image.metadata().derived.saturation_flagged(),
            conditions: CaptureConditions::of(image.metadata()),
            dark_comparison: image.metadata().derived.dark_comparison(),
        }
    }
}
