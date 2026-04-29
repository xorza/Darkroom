use egui::{Rect, Response, Sense};

use crate::common::StableId;
use crate::gui::Gui;

/// A reservation for pointer interaction over a rectangle. Wraps egui's
/// `Ui::interact` so app-layer code never touches raw `egui::Ui`.
///
/// Defaults: rect = `Rect::NOTHING` (no geometry, still returns a usable
/// `Response`), sense = `Sense::hover()`.
///
/// Two terminal forms:
/// - [`HitRegion::show`] returns the bare [`Response`]. Use for
///   passive hit-tests, full-container input absorbers, etc.
/// - [`HitRegion::show_positioned`] is the input layer for a
///   positioned widget that paints at the rect: checks
///   `is_rect_visible`, downgrades sense to `hover()` when off-screen,
///   and returns the rect + a `visible` flag so the caller can skip
///   painting.
#[derive(Debug)]
pub struct HitRegion {
    id: StableId,
    rect: Rect,
    sense: Sense,
}

/// Output of [`HitRegion::show_culled`]. `visible == false` means the
/// rect is outside the parent's clip region — caller should skip
/// painting; `response` is registered with `Sense::hover()` regardless
/// of the requested sense so the widget id is still claimed for the
/// frame.
#[derive(Debug)]
pub struct HitOutput {
    pub rect: Rect,
    pub response: Response,
    pub visible: bool,
}

impl HitRegion {
    pub fn new(id: StableId) -> Self {
        Self {
            id,
            rect: Rect::NOTHING,
            sense: Sense::hover(),
        }
    }

    pub fn rect(mut self, rect: Rect) -> Self {
        self.rect = rect;
        self
    }

    pub fn sense(mut self, sense: Sense) -> Self {
        self.sense = sense;
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        gui.ui_raw().interact(self.rect, self.id.id(), self.sense)
    }

    /// Input layer for a positioned widget that paints at the rect.
    /// Same as [`Self::show`] but additionally:
    /// - asserts the rect has finite coordinates,
    /// - checks `is_rect_visible`; if not visible, downgrades sense to
    ///   `hover()` so the rect doesn't soak input from things below it,
    /// - returns the rect alongside the response with a `visible` flag.
    pub fn show_positioned(self, gui: &mut Gui<'_>) -> HitOutput {
        assert!(self.rect.min.x.is_finite() && self.rect.min.y.is_finite());
        assert!(self.rect.max.x.is_finite() && self.rect.max.y.is_finite());
        let visible = gui.ui_raw().is_rect_visible(self.rect);
        let sense = if visible { self.sense } else { Sense::hover() };
        let response = gui.ui_raw().interact(self.rect, self.id.id(), sense);
        HitOutput {
            rect: self.rect,
            response,
            visible,
        }
    }
}
