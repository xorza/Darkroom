//! Circular toggle within an exclusive group: clicking sets
//! `*current = value`, painting an inner yellow disc when selected.
//! Render path mirrors [`Checkbox`] — the only differences are the
//! geometry (circle, not rect) and the selection model: instead of
//! flipping a bool, the widget assigns `value` into a caller-owned
//! slot, so multiple `RadioButton`s wired to the same slot become an
//! exclusive group naturally.

use egui::{Response, Sense, Vec2, vec2};

use crate::common::StableId;
use crate::gui::Gui;

#[must_use = "RadioButton does nothing until .show() is called"]
pub struct RadioButton<'a, T: PartialEq> {
    id: StableId,
    current: &'a mut T,
    value: T,
    text: Option<&'a str>,
    enabled: bool,
}

impl<'a, T: PartialEq + Clone> RadioButton<'a, T> {
    pub fn new(id: StableId, current: &'a mut T, value: T) -> Self {
        Self {
            id,
            current,
            value,
            text: None,
            enabled: true,
        }
    }

    pub fn text(mut self, text: &'a str) -> Self {
        self.text = Some(text);
        self
    }

    #[allow(dead_code)]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        let id = self.id;
        let selected = *self.current == self.value;

        let font = gui.style.sub_font.clone();
        let text_color = if self.enabled {
            gui.style.text_color
        } else {
            gui.style.noninteractive_text_color
        };
        let galley = self.text.map(|t| gui.layout_no_wrap(t, &font, text_color));

        let box_size = gui.font_height(&font);
        let gap = gui.style.padding;
        let total_size = match galley.as_ref() {
            Some(g) => vec2(box_size + gap + g.size().x, box_size.max(g.size().y)),
            None => Vec2::splat(box_size),
        };

        let sense = if self.enabled {
            Sense::click() | Sense::hover()
        } else {
            Sense::hover()
        };
        let (rect, response) = gui.scope(id).autosize(total_size, sense);
        if !gui.ui_raw().is_rect_visible(rect) {
            return response;
        }

        if response.clicked() && self.enabled {
            *self.current = self.value.clone();
        }

        let outer_radius = box_size * 0.5;
        let center = egui::pos2(rect.min.x + outer_radius, rect.center().y);

        let outer_fill = if !self.enabled {
            gui.style.noninteractive_bg_fill
        } else if response.is_pointer_button_down_on() {
            gui.style.active_bg_fill
        } else if response.hovered() {
            gui.style.hover_bg_fill
        } else {
            gui.style.inactive_bg_fill
        };
        let outer_stroke = if response.hovered() && self.enabled {
            gui.style.active_bg_stroke
        } else {
            gui.style.inactive_bg_stroke
        };
        gui.painter()
            .circle(center, outer_radius, outer_fill, outer_stroke);

        if selected {
            let inner_radius = (outer_radius - gui.style.small_padding).max(1.0);
            gui.painter()
                .circle_filled(center, inner_radius, gui.style.checked_bg_fill);
        }

        if let Some(galley) = galley {
            let text_pos = egui::pos2(
                rect.min.x + box_size + gap,
                rect.center().y - galley.size().y * 0.5,
            );
            gui.painter().galley(text_pos, galley, text_color);
        }

        response
    }
}
