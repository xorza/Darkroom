//! Record-time visibility culling for the graph canvas. Only items that
//! intersect the viewport are recorded, so an off-screen
//! graph costs no measure/arrange/paint work. Pure world-space math;
//! [`crate::gui::canvas::GraphUI::frame`] resolves one [`CullRegion`] per
//! frame and threads the same policy through every recorded canvas item.

use glam::Vec2;
use palantir::{Rect, Size};

use crate::core::document::Viewport;
use crate::gui::canvas::to_world;
use crate::gui::canvas::wire::Wire;

/// World-space slack added around the viewport so paint that extends past
/// an element's layout rect (status-glow shadow, wire stroke width,
/// selection border) never pops at the screen edge.
const CULL_MARGIN: f32 = 16.0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct CullRegion {
    visible: Option<Rect>,
}

impl CullRegion {
    pub(super) fn from_canvas(
        outer_screen: Option<Rect>,
        canvas_origin: Vec2,
        viewport: &Viewport,
    ) -> Self {
        let visible = outer_screen.map(|outer| {
            let outer_local = Rect {
                min: outer.min - canvas_origin,
                size: outer.size,
            };
            let min = to_world(outer_local.min, viewport);
            let max = to_world(outer_local.max(), viewport);
            Rect {
                min,
                size: Size::new(max.x - min.x, max.y - min.y),
            }
            .inflated(CULL_MARGIN)
        });
        Self { visible }
    }

    /// Unmeasured nodes stay recorded until their size becomes known.
    pub(crate) fn keeps_node(self, rect: Option<Rect>) -> bool {
        rect.is_none_or(|rect| self.keeps_rect(rect))
    }

    pub(super) fn keeps_wire(self, wire: &Wire) -> bool {
        self.keeps_rect(wire.hull())
    }

    /// A pinned output stays recorded while *either* half of its glyph is
    /// visible: the preview card or the bezier reaching back to its port.
    pub(super) fn keeps_pin(self, card: Rect, wire: &Wire) -> bool {
        self.keeps_rect(card) || self.keeps_wire(wire)
    }

    fn keeps_rect(self, rect: Rect) -> bool {
        self.visible.is_none_or(|visible| visible.intersects(rect))
    }
}

#[cfg(test)]
mod tests;
