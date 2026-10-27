//! [`Tie`]: a cable tie that gathers wires into a ribbon on the canvas.

use common::Id;
use glam::Vec2;
use serde::{Deserialize, Serialize};

/// A tie's id, unique across a document.
pub(crate) type TieId = Id<Tie>;

/// Where a tie sits and how its band turns. Layout only: compile, plan and run never read it.
/// The wires it holds are the routes that name it, in
/// [`GraphView::routes`](crate::core::document::graph_view::GraphView::routes).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tie {
    /// Canvas-world position of the band's center.
    pub(crate) pos: Vec2,
    pub(crate) angle: TieAngle,
}

/// How a tie's band turns.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum TieAngle {
    /// Square to the mean direction of its strands, derived each frame.
    Auto,
    /// The band's angle, in degrees from `+x` toward `+y`, in `[0, 360)`; the strands cross it
    /// square.
    Pinned(f32),
}

impl TieAngle {
    /// Whether a pinned angle is in `[0, 360)`, which refuses a non-finite one too.
    pub(crate) fn is_valid(self) -> bool {
        match self {
            Self::Auto => true,
            Self::Pinned(degrees) => (0.0..360.0).contains(&degrees),
        }
    }
}
