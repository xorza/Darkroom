use eframe::egui;
use egui::{Align2, Color32, Rect, Response, Sense, Stroke, StrokeKind};

use crate::gui::Gui;

#[derive(Debug, Clone, Copy)]
pub struct ToggleButtonBackground {
    fill: Color32,
    inactive_stroke: Stroke,
    hovered_stroke: Stroke,
    radius: f32,
}

#[derive(Debug)]
pub struct ToggleButton<'a> {
    enabled: bool,
    value: &'a mut bool,
    text: Option<&'a str>,
    tooltip: Option<&'a str>,
    background: Option<ToggleButtonBackground>,
    rect: Option<Rect>,
}

impl<'a> ToggleButton<'a> {
    pub fn new(value: &'a mut bool) -> Self {
        Self {
            enabled: true,
            value,
            text: None,
            tooltip: None,
            background: None,
            rect: None,
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

    pub fn background(mut self, background: ToggleButtonBackground) -> Self {
        assert!(background.radius.is_finite());
        self.background = Some(background);
        self
    }

    pub fn rect(mut self, rect: Rect) -> Self {
        self.rect = Some(rect);
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        let text = self.text.unwrap_or("");

        let text_color = if !self.enabled {
            gui.style.noninteractive_text_color
        } else if *self.value {
            gui.style.dark_text_color
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

        if response.clicked() && self.enabled {
            *self.value = !*self.value;
        }

        if response.hovered()
            && let Some(tooltip) = self.tooltip
            && !tooltip.is_empty()
        {
            response.show_tooltip_text(tooltip);
        }

        let default_fill = if !self.enabled {
            gui.style.noninteractive_bg_fill
        } else if *self.value {
            gui.style.checked_bg_fill
        } else if response.is_pointer_button_down_on() {
            gui.style.active_bg_fill
        } else if response.hovered() {
            gui.style.hover_bg_fill
        } else {
            gui.style.inactive_bg_fill
        };

        let background = self.background.unwrap_or(ToggleButtonBackground {
            fill: default_fill,
            inactive_stroke: gui.style.inactive_bg_stroke,
            hovered_stroke: gui.style.active_bg_stroke,
            radius: gui.style.small_corner_radius,
        });

        let stroke = if response.hovered() && self.enabled {
            background.hovered_stroke
        } else {
            background.inactive_stroke
        };

        gui.painter().rect(
            rect,
            background.radius,
            background.fill,
            stroke,
            StrokeKind::Inside,
        );
        let text_pos = rect.min + (rect.size() - galley.size()) * 0.5;
        gui.painter()
            .galley(text_pos, galley.clone(), gui.style.text_color);

        response
    }
}
