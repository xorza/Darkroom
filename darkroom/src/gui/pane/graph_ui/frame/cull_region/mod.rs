//! Record-time visibility culling for the graph canvas. Only items that
//! intersect the viewport are recorded, so an off-screen
//! graph costs no measure/arrange/paint work. Pure world-space math;
//! [`crate::gui::pane::graph_ui::GraphUi::draw`] resolves one [`CullRegion`] per
//! frame and threads the same policy through every recorded canvas item.

use glam::Vec2;
use palantir::{Rect, Size};

use crate::core::document::viewport::Viewport;

/// World-space slack added around the viewport so paint that extends past
/// an element's layout rect (status-glow shadow, wire stroke width,
/// selection border) never pops at the screen edge.
const CULL_MARGIN: f32 = 16.0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct CullRegion {
    visible: Option<Rect>,
}

impl CullRegion {
    /// The region `viewport` shows of a canvas of `size`, which keeps everything while the
    /// canvas has not been measured. The inner canvas fills the outer one, so their local
    /// spaces share an origin.
    pub(crate) fn from_size(size: Option<Size>, viewport: &Viewport) -> Self {
        let visible = size.map(|size| {
            let min = viewport.to_world(Vec2::ZERO);
            let max = viewport.to_world(Vec2::new(size.w, size.h));
            Rect::from_min_max(min, max).inflated(CULL_MARGIN)
        });
        Self { visible }
    }

    /// Unmeasured nodes stay recorded until their size becomes known.
    pub(crate) fn keeps_node(self, rect: Option<Rect>) -> bool {
        rect.is_none_or(|rect| self.keeps_rect(rect))
    }

    pub(crate) fn keeps_rect(self, rect: Rect) -> bool {
        self.visible.is_none_or(|visible| visible.intersects(rect))
    }
}

#[cfg(test)]
mod tests;
