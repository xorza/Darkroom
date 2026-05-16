use egui::{Rect, Response, Stroke, Vec2, pos2};

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::widgets::Button;

/// Square button painted with two diagonal strokes forming an `×`.
/// Used for "remove this thing" actions: node delete, modal close, etc.
/// Colours and chrome inherit from [`Button`]; only the glyph is custom
/// so the result tracks the active button style.
#[must_use = "CloseButton does nothing until .show() is called"]
pub struct CloseButton<'a> {
    id: StableId,
    rect: Option<Rect>,
    tooltip: Option<&'a str>,
}

impl<'a> CloseButton<'a> {
    pub fn new(id: StableId) -> Self {
        Self {
            id,
            rect: None,
            tooltip: None,
        }
    }

    /// Pin the button to an explicit rect. Without this the button
    /// auto-sizes to a `row_height × row_height` square in the parent
    /// layout's flow.
    pub fn rect(mut self, rect: Rect) -> Self {
        self.rect = Some(rect);
        self
    }

    pub fn tooltip(mut self, tooltip: &'a str) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        let mut button = Button::new(self.id);
        if let Some(rect) = self.rect {
            button = button.rect(rect);
        } else {
            let side = gui.style.row_height;
            button = button.size(Vec2::splat(side));
        }
        if let Some(tooltip) = self.tooltip {
            button = button.tooltip(tooltip);
        }
        let response = button.show(gui);

        let rect = response.rect;
        let margin = rect.width() * 0.3;
        let tl = pos2(rect.min.x + margin, rect.min.y + margin);
        let br = pos2(rect.max.x - margin, rect.max.y - margin);
        let bl = pos2(rect.min.x + margin, rect.max.y - margin);
        let tr = pos2(rect.max.x - margin, rect.min.y + margin);
        let stroke = Stroke::new(1.4 * gui.scale(), gui.style.text_color);
        gui.painter().line_segment([tl, br], stroke);
        gui.painter().line_segment([bl, tr], stroke);

        response
    }
}
