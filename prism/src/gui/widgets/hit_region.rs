use egui::{Rect, Response, Sense};

use crate::common::StableId;
use crate::gui::Gui;

/// A reservation for pointer interaction over a rectangle. Wraps egui's
/// `Ui::interact` so app-layer code never touches raw `egui::Ui`.
///
/// Defaults: rect = `Rect::NOTHING` (no geometry, still returns a usable
/// `Response`), sense = `Sense::hover()`.
#[derive(Debug)]
pub struct HitRegion {
    id: StableId,
    rect: Rect,
    sense: Sense,
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
}
