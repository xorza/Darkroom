//! Panning and zooming the graph canvas.

use serde::{Deserialize, Serialize};

use crate::core::document::{Document, Viewport};
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;

/// 1e-4 is the threshold below which two pan/scale samples are considered the
/// same camera — it keeps idle pan/zoom from polluting the undo stack with
/// sub-pixel deltas.
const VIEWPORT_EPS: f32 = 1e-4;

/// The graph camera, before and after.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetViewport {
    pub(crate) viewport: Change<Viewport>,
}

impl SetViewport {
    /// Fold `next`, a later frame of the same gesture, into this step: keep
    /// the camera it started from and adopt the latest.
    pub(crate) const fn absorb(&mut self, next: &Self) {
        self.viewport.to = next.viewport.to;
    }
}

impl Reversible for SetViewport {
    fn write(&self, doc: &mut Document, dir: Direction) {
        doc.main_view.viewport = *self.viewport.half(dir);
    }

    /// Compared with a tolerance rather than for equality: a pan that moved a
    /// thousandth of a pixel is the same camera, and recording it would put a
    /// Ctrl+Z between the user and their last real edit.
    fn is_noop(&self) -> bool {
        let Change { from, to } = &self.viewport;
        (from.pan - to.pan).length_squared() < VIEWPORT_EPS * VIEWPORT_EPS
            && (from.zoom - to.zoom).abs() < VIEWPORT_EPS
    }

    fn dirties_document(&self) -> bool {
        false
    }

    /// The viewport is the inner canvas's paint-time `TranslateScale`;
    /// children arrange in pre-transform space, so a pan or zoom changes
    /// nothing the layout engine reads.
    fn invalidates_cached_geometry(&self) -> bool {
        false
    }
}
