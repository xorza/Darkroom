use eframe::egui;
use egui::{Align2, Color32, Rect, Response, Sense, Shape, Stroke, StrokeKind};

use crate::gui::Gui;

#[derive(Debug, Clone, Copy)]
pub struct ButtonBackground {
    disabled_fill: Color32,
    idle_fill: Color32,
    hover_fill: Color32,
    active_fill: Color32,
    inactive_stroke: Stroke,
    hovered_stroke: Stroke,
    radius: f32,
}

#[derive(Debug)]
pub struct Button<'a> {
    enabled: bool,
    text: Option<&'a str>,
    tooltip: Option<&'a str>,
    background: Option<ButtonBackground>,
    rect: Option<Rect>,
    shapes: Vec<Shape>,
}

impl<'a> Button<'a> {
    pub fn new() -> Self {
        Self {
            enabled: true,
            text: None,
            tooltip: None,
            background: None,
            rect: None,
            shapes: Vec::new(),
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn text(mut self, text: &'a str) -> Self {
        self.text = Some(text);
        self
    }

    pub fn tooltip(mut self, tooltip: &'a str) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    pub fn background(mut self, background: ButtonBackground) -> Self {
        assert!(background.radius.is_finite());
        self.background = Some(background);
        self
    }

    pub fn rect(mut self, rect: Rect) -> Self {
        self.rect = Some(rect);
        self
    }

    pub fn shapes(mut self, shapes: impl IntoIterator<Item = impl Into<Shape>>) -> Self {
        self.shapes = shapes.into_iter().map(Into::into).collect();
        self
    }

    pub fn show(self, gui: &mut Gui<'_>, _id_salt: impl std::hash::Hash) -> Response {
        let text = self.text.unwrap_or("");

        let text_color = if !self.enabled {
            gui.style.noninteractive_text_color
        } else {
            gui.style.text_color
        };
        let galley =
            gui.painter()
                .layout_no_wrap(text.to_string(), gui.style.sub_font.clone(), text_color);

        let sense = if self.enabled {
            Sense::click() | Sense::hover()
        } else {
            Sense::hover()
        };

        let (rect, response) = if let Some(rect) = self.rect {
            let response = gui.ui().allocate_rect(rect, sense);
            (rect, response)
        } else {
            // Autosize: calculate button size based on text
            let padding = gui.style.small_padding * 2.0;
            let text_size = galley.size();
            let button_size = egui::vec2(text_size.x + padding, text_size.y + padding);
            let (rect, response) = gui.ui().allocate_exact_size(button_size, sense);
            (rect, response)
        };

        if response.hovered()
            && let Some(tooltip) = self.tooltip
            && !tooltip.is_empty()
        {
            response.show_tooltip_text(tooltip);
        }

        let background = self.background.unwrap_or(ButtonBackground {
            disabled_fill: gui.style.noninteractive_bg_fill,
            idle_fill: gui.style.inactive_bg_fill,
            hover_fill: gui.style.hover_bg_fill,
            active_fill: gui.style.active_bg_fill,
            inactive_stroke: gui.style.inactive_bg_stroke,
            hovered_stroke: gui.style.active_bg_stroke,
            radius: gui.style.small_corner_radius,
        });

        let fill = if !self.enabled {
            background.disabled_fill
        } else if response.is_pointer_button_down_on() {
            background.active_fill
        } else if response.hovered() {
            background.hover_fill
        } else {
            background.idle_fill
        };

        let stroke = if response.hovered() && self.enabled {
            background.hovered_stroke
        } else {
            background.inactive_stroke
        };

        gui.painter()
            .rect(rect, background.radius, fill, stroke, StrokeKind::Middle);

        if !self.shapes.is_empty() {
            gui.painter().extend(self.shapes);
        } else {
            let text_pos = rect.min + (rect.size() - galley.size()) * 0.5;
            gui.painter().galley(text_pos, galley, text_color);
        }

        response
    }
}
