//! Square toggle: outline outer, yellow inner rect when checked.
//! Render path mirrors [`Button`] (autosize via `gui.scope().autosize`,
//! style fields for fill/stroke, click flips the bound bool) but draws
//! a contrasting inner rect — the same yellow as the cache toggle's
//! checked fill — instead of recolouring the whole outer rect.
//!
//! Pairs an optional text label to the right of the box. Without a
//! label the widget collapses to just the box.

use egui::{Response, Sense, StrokeKind, Vec2, vec2};

use crate::common::StableId;
use crate::gui::Gui;

#[must_use = "Checkbox does nothing until .show() is called"]
pub struct Checkbox<'a> {
    id: StableId,
    value: &'a mut bool,
    text: Option<&'a str>,
    enabled: bool,
}

impl<'a> Checkbox<'a> {
    pub fn new(id: StableId, value: &'a mut bool) -> Self {
        Self {
            id,
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
        let checked = *self.value;

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
            *self.value = !*self.value;
        }

        // Box rect: square pinned to the left, vertically centered.
        let box_min = egui::pos2(rect.min.x, rect.center().y - box_size * 0.5);
        let box_rect = egui::Rect::from_min_size(box_min, Vec2::splat(box_size));

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
        gui.painter().rect(
            box_rect,
            gui.style.small_corner_radius,
            outer_fill,
            outer_stroke,
            StrokeKind::Inside,
        );

        if checked {
            let inner = box_rect.shrink(gui.style.small_padding);
            gui.painter().rect_filled(
                inner,
                gui.style.small_corner_radius,
                gui.style.checked_bg_fill,
            );
        }

        if let Some(galley) = galley {
            let text_pos = egui::pos2(
                box_rect.max.x + gap,
                rect.center().y - galley.size().y * 0.5,
            );
            gui.painter().galley(text_pos, galley, text_color);
        }

        response
    }
}
