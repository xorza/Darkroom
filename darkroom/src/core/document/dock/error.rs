//! What [`DockLayout::validate`](crate::core::document::dock::DockLayout::validate)
//! rejects: a tree that broke one of the invariants the module doc lists.

use crate::core::document::TabRef;
use crate::core::document::dock::TabGroupId;

#[derive(Debug, thiserror::Error)]
pub(crate) enum DockValidationError {
    #[error("dock nodes are not in canonical pre-order")]
    NonCanonical,
    #[error("dock node index {index} out of range")]
    NodeOutOfRange { index: u32 },
    #[error("split nesting exceeds the cap")]
    SplitNesting,
    #[error("split ratio {ratio} out of bounds")]
    SplitRatio { ratio: f32 },
    #[error("dock tree has slots unreachable from the root")]
    UnreachableSlots,
    #[error("no group holds the Main graph tab")]
    MissingMainTab,
    #[error("dock group id {group_id:?} appears twice")]
    DuplicateGroup { group_id: TabGroupId },
    #[error("dock group {group_id:?} is empty")]
    EmptyGroup { group_id: TabGroupId },
    #[error("dock group {group_id:?} active tab out of range")]
    ActiveTabOutOfRange { group_id: TabGroupId },
    #[error("tab {tab:?} appears twice")]
    DuplicateTab { tab: TabRef },
    #[error("focused group {group_id:?} is missing")]
    MissingFocusedGroup { group_id: TabGroupId },
}
