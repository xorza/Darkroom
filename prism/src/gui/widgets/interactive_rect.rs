use egui::{Rect, Response, Sense};

use crate::common::StableId;
use crate::gui::Gui;

/// Helper for widgets laid out at a caller-computed `rect`. Centralizes
/// off-screen culling and the single `interact` registration that the
/// positioned-widget family used to duplicate.
#[derive(Debug)]
#[must_use = "InteractiveRect does nothing until .show() is called"]
pub struct InteractiveRect {
    id: StableId,
    rect: Rect,
    sense: Sense,
}

#[derive(Debug)]
pub struct InteractiveRectOutput {
    pub rect: Rect,
    pub response: Response,
    /// `false` when the rect is outside the visible region. Caller
    /// should skip painting in that case; `response` is registered
    /// with `Sense::hover()` regardless of the requested sense so the
    /// widget id is still claimed for the frame.
    pub visible: bool,
}

impl InteractiveRect {
    pub fn new(id: StableId, rect: Rect) -> Self {
        assert!(rect.min.x.is_finite() && rect.min.y.is_finite());
        assert!(rect.max.x.is_finite() && rect.max.y.is_finite());
        Self {
            id,
            rect,
            sense: Sense::hover(),
        }
    }

    pub fn sense(mut self, sense: Sense) -> Self {
        self.sense = sense;
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> InteractiveRectOutput {
        let visible = gui.ui_raw().is_rect_visible(self.rect);
        let sense = if visible { self.sense } else { Sense::hover() };
        let response = gui.ui_raw().interact(self.rect, self.id.id(), sense);
        InteractiveRectOutput {
            rect: self.rect,
            response,
            visible,
        }
    }
}
