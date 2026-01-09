use eframe::egui;
use egui::{Align2, Color32, Rect, Response, Sense, Stroke, StrokeKind};

use crate::gui::Gui;

#[derive(Debug, Clone, Copy)]
struct ToggleButtonBackground {
    fill: Color32,
    stroke: Stroke,
    radius: f32,
}

#[derive(Debug)]
pub struct ToggleButton<'a> {
    id: egui::Id,
    enabled: bool,
    checked: bool,
    text: &'a str,
    tooltip: Option<&'a str>,
    background: Option<ToggleButtonBackground>,
}

impl<'a> ToggleButton<'a> {
    pub fn new(id: egui::Id, text: &'a str) -> Self {
        Self {
            id,
            enabled: true,
            checked: false,
            text,
            tooltip: None,
            background: None,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn tooltip(mut self, tooltip: &'a str) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    pub fn background(mut self, fill: Color32, stroke: Stroke, radius: f32) -> Self {
        assert!(radius.is_finite());
        self.background = Some(ToggleButtonBackground {
            fill,
            stroke,
            radius,
        });
        self
    }

    pub fn show(self, gui: &mut Gui<'_>, rect: Rect) -> Response {
        let response = gui.ui().interact(
            rect,
            self.id,
            if self.enabled {
                Sense::click() | Sense::hover()
            } else {
                Sense::hover()
            },
        );
        if response.hovered()
            && let Some(tooltip) = self.tooltip
            && !tooltip.is_empty()
        {
            response.show_tooltip_text(tooltip);
        }

        let text_color = if !self.enabled {
            gui.style.noninteractive_text_color
        } else if self.checked {
            gui.style.checked_text_color
        } else {
            gui.style.text_color
        };
        let default_fill = if !self.enabled {
            gui.style.noninteractive_bg_fill
        } else if self.checked {
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
            stroke: gui.style.inactive_bg_stroke,
            radius: gui.style.small_corner_radius,
        });

        gui.painter().rect(
            rect,
            background.radius,
            background.fill,
            background.stroke,
            StrokeKind::Middle,
        );
        gui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            self.text,
            gui.style.sub_font.clone(),
            text_color,
        );

        response
    }
}
