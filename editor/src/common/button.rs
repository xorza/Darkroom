use eframe::egui;
use egui::{Color32, Rect, Response, Sense, Shape, Stroke, StrokeKind};

use crate::gui::Gui;

#[derive(Debug, Clone, Copy)]
struct ButtonBackground {
    disabled_fill: Color32,
    idle_fill: Color32,
    hover_fill: Color32,
    active_fill: Color32,
    stroke: Stroke,
    radius: f32,
}

#[derive(Debug)]
pub struct Button<'a> {
    id: egui::Id,
    enabled: bool,
    tooltip: Option<&'a str>,
    background: Option<ButtonBackground>,
}

impl<'a> Button<'a> {
    pub fn new(id: egui::Id) -> Self {
        Self {
            id,
            enabled: true,
            tooltip: None,
            background: None,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn tooltip(mut self, tooltip: &'a str) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    pub fn background(
        mut self,
        disabled_fill: Color32,
        idle_fill: Color32,
        hover_fill: Color32,
        active_fill: Color32,
        stroke: Stroke,
        radius: f32,
    ) -> Self {
        assert!(radius.is_finite());
        self.background = Some(ButtonBackground {
            disabled_fill,
            idle_fill,
            hover_fill,
            active_fill,
            stroke,
            radius,
        });
        self
    }

    pub fn show(
        self,
        gui: &mut Gui<'_>,
        rect: Rect,
        shapes: impl IntoIterator<Item = impl Into<Shape>>,
    ) -> Response {
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

        let background = self.background.unwrap_or(ButtonBackground {
            disabled_fill: gui.style.noninteractive_bg_fill,
            idle_fill: gui.style.inactive_bg_fill,
            hover_fill: gui.style.hover_bg_fill,
            active_fill: gui.style.active_bg_fill,
            stroke: gui.style.inactive_bg_stroke,
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

        gui.painter().rect(
            rect,
            background.radius,
            fill,
            background.stroke,
            StrokeKind::Middle,
        );
        gui.painter().extend(shapes.into_iter().map(Into::into));

        response
    }
}
