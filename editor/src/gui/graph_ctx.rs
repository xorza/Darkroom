use eframe::egui;
use egui::{Color32, CornerRadius, FontId, Painter, Rect, Response, Stroke, StrokeKind, Ui};
use graph::prelude::FuncLib;

use crate::{common::font::ScaledFontId, gui::style::Style};

pub struct GraphContext<'a> {
    pub ui: &'a mut Ui,
    pub painter: Painter,
    pub rect: Rect,
    pub style: Style,
    pub scale: f32,

    pub func_lib: &'a FuncLib,
}

impl<'a> std::fmt::Debug for GraphContext<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphContext")
            .field("rect", &self.rect)
            .field("style", &self.style)
            .field("scale", &self.scale)
            .field("func_lib", &"FuncLib")
            .finish()
    }
}

impl<'a> GraphContext<'a> {
    pub fn new(ui: &'a mut Ui, func_lib: &'a FuncLib, scale: f32) -> Self {
        let style = Style::new();
        let rect = ui.available_rect_before_wrap();
        let painter = ui.painter_at(rect);

        Self {
            ui,
            painter,
            rect,
            style,
            scale,
            func_lib,
        }
    }

    pub fn toggle_button(
        &mut self,
        rect: Rect,
        text: &str,
        enabled: bool,
        checked: bool,
        id: impl std::hash::Hash,
    ) -> bool {
        let id = self.ui.auto_id_with(id);
        let response = self.ui.interact(
            rect,
            id,
            if enabled {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
        );
        let fill = if !enabled {
            self.style.widget_noninteractive_bg_fill
        } else if checked {
            self.style.cache_active_color
        } else if response.is_pointer_button_down_on() {
            self.style.widget_active_bg_fill
        } else if response.hovered() {
            self.style.widget_hover_bg_fill
        } else {
            self.style.widget_inactive_bg_fill
        };
        let stroke = self.style.widget_inactive_bg_stroke;

        self.painter.rect(
            rect,
            self.style.node_corner_radius * self.scale,
            fill,
            stroke,
            StrokeKind::Middle,
        );

        let button_text_color = if !enabled {
            self.style.widget_noninteractive_text_color
        } else if checked {
            self.style.cache_checked_text_color
        } else {
            self.style.widget_text_color
        };
        self.painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            self.style.body_font.scaled(self.scale),
            button_text_color,
        );

        response.clicked()
    }
}
