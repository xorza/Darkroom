use common::BoolExt;
use eframe::egui;
use egui::{Align2, Painter, Rect, Sense, Shape, StrokeKind, Ui};
use graph::prelude::FuncLib;

use crate::{common::font::ScaledFontId, gui::style::Style};

pub struct GraphContext<'a> {
    pub ui: &'a mut Ui,
    pub painter: Painter,
    pub rect: Rect,
    pub style: Style,
    pub scale: f32,

    pub func_lib: &'a FuncLib,

    pub arena: bumpalo::Bump,
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
            arena: bumpalo::Bump::new(),
        }
    }

    pub fn toggle_button(
        &mut self,
        rect: Rect,
        enabled: bool,
        checked: bool,
        id_source: impl std::hash::Hash,
        text: &str,
        tooltip: &str,
    ) -> bool {
        let id = self.ui.make_persistent_id(id_source);
        let response = self.ui.interact(
            rect,
            id,
            enabled.then_else(Sense::click() | Sense::hover(), Sense::hover()),
        );
        if response.hovered() && !tooltip.is_empty() {
            response.show_tooltip_text(tooltip);
        }

        let text_color = if !enabled {
            self.style.noninteractive_text_color
        } else if checked {
            self.style.checked_text_color
        } else {
            self.style.text_color
        };
        let fill = if !enabled {
            self.style.noninteractive_bg_fill
        } else if checked {
            self.style.checked_bg_fill
        } else if response.is_pointer_button_down_on() {
            self.style.active_bg_fill
        } else if response.hovered() {
            self.style.hover_bg_fill
        } else {
            self.style.inactive_bg_fill
        };
        let stroke = self.style.inactive_bg_stroke;

        self.painter.rect(
            rect,
            self.style.small_corner_radius * self.scale,
            fill,
            stroke,
            StrokeKind::Middle,
        );
        self.painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            text,
            self.style.sub_font.scaled(self.scale),
            text_color,
        );

        response.clicked()
    }

    pub fn toggle_button_with(
        &mut self,
        rect: Rect,
        enabled: bool,
        checked: bool,
        id_source: impl std::hash::Hash,
        shapes: impl IntoIterator<Item = impl Into<Shape>>,
        tooltip: &str,
    ) -> bool {
        let id = self.ui.make_persistent_id(id_source);
        let response = self.ui.interact(
            rect,
            id,
            enabled.then_else(Sense::click() | Sense::hover(), Sense::hover()),
        );
        if response.hovered() && !tooltip.is_empty() {
            response.show_tooltip_text(tooltip);
        }

        let fill = if !enabled {
            self.style.noninteractive_bg_fill
        } else if checked {
            self.style.checked_bg_fill
        } else if response.is_pointer_button_down_on() {
            self.style.active_bg_fill
        } else if response.hovered() {
            self.style.hover_bg_fill
        } else {
            self.style.inactive_bg_fill
        };
        let stroke = self.style.inactive_bg_stroke;

        self.painter.rect(
            rect,
            self.style.small_corner_radius * self.scale,
            fill,
            stroke,
            StrokeKind::Middle,
        );
        let shapes = shapes.into_iter().map(Into::into);
        self.painter.extend(shapes);

        response.clicked()
    }

    pub fn button_with(
        &mut self,
        rect: Rect,
        enabled: bool,
        id_source: impl std::hash::Hash,
        shapes: impl IntoIterator<Item = Shape>,
        tooltip: &str,
    ) -> bool {
        let id = self.ui.make_persistent_id(id_source);
        let response = self.ui.interact(
            rect,
            id,
            enabled.then_else(Sense::click() | Sense::hover(), Sense::hover()),
        );
        if response.hovered() && !tooltip.is_empty() {
            response.show_tooltip_text(tooltip);
        }
        let fill = if !enabled {
            self.style.noninteractive_bg_fill
        } else if response.is_pointer_button_down_on() {
            self.style.active_bg_fill
        } else if response.hovered() {
            self.style.hover_bg_fill
        } else {
            self.style.inactive_bg_fill
        };
        let stroke = self.style.inactive_bg_stroke;

        self.painter.rect(
            rect,
            self.style.small_corner_radius * self.scale,
            fill,
            stroke,
            StrokeKind::Middle,
        );
        self.painter.extend(shapes);

        response.clicked()
    }
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
