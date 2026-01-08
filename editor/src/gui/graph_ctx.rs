use common::BoolExt;
use eframe::egui;
use egui::{Align2, Rect, Sense, Shape, StrokeKind};
use graph::prelude::FuncLib;

use crate::{common::font::ScaledFontId, gui::Gui};

pub struct GraphContext<'a> {
    pub func_lib: &'a FuncLib,
}

impl<'a> GraphContext<'a> {
    pub fn new(func_lib: &'a FuncLib) -> Self {
        Self { func_lib }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn toggle_button(
        &mut self,
        gui: &mut Gui<'_>,
        rect: Rect,
        enabled: bool,
        checked: bool,
        id_source: impl std::hash::Hash,
        text: &str,
        tooltip: &str,
    ) -> bool {
        let id = gui.ui().make_persistent_id(id_source);
        let response = gui.ui().interact(
            rect,
            id,
            enabled.then_else(Sense::click() | Sense::hover(), Sense::hover()),
        );
        if response.hovered() && !tooltip.is_empty() {
            response.show_tooltip_text(tooltip);
        }

        let text_color = if !enabled {
            gui.style.noninteractive_text_color
        } else if checked {
            gui.style.checked_text_color
        } else {
            gui.style.text_color
        };
        let fill = if !enabled {
            gui.style.noninteractive_bg_fill
        } else if checked {
            gui.style.checked_bg_fill
        } else if response.is_pointer_button_down_on() {
            gui.style.active_bg_fill
        } else if response.hovered() {
            gui.style.hover_bg_fill
        } else {
            gui.style.inactive_bg_fill
        };
        let stroke = gui.style.inactive_bg_stroke;

        gui.painter().rect(
            rect,
            gui.style.small_corner_radius * gui.scale,
            fill,
            stroke,
            StrokeKind::Middle,
        );
        gui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            text,
            gui.style.sub_font.scaled(gui.scale),
            text_color,
        );

        response.clicked()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn toggle_button_with(
        &mut self,
        gui: &mut Gui<'_>,
        rect: Rect,
        enabled: bool,
        checked: bool,
        id_source: impl std::hash::Hash,
        shapes: impl IntoIterator<Item = impl Into<Shape>>,
        tooltip: &str,
    ) -> bool {
        let id = gui.ui().make_persistent_id(id_source);
        let response = gui.ui().interact(
            rect,
            id,
            enabled.then_else(Sense::click() | Sense::hover(), Sense::hover()),
        );
        if response.hovered() && !tooltip.is_empty() {
            response.show_tooltip_text(tooltip);
        }

        let fill = if !enabled {
            gui.style.noninteractive_bg_fill
        } else if checked {
            gui.style.checked_bg_fill
        } else if response.is_pointer_button_down_on() {
            gui.style.active_bg_fill
        } else if response.hovered() {
            gui.style.hover_bg_fill
        } else {
            gui.style.inactive_bg_fill
        };
        let stroke = gui.style.inactive_bg_stroke;

        gui.painter().rect(
            rect,
            gui.style.small_corner_radius * gui.scale,
            fill,
            stroke,
            StrokeKind::Middle,
        );
        let shapes = shapes.into_iter().map(Into::into);
        gui.painter().extend(shapes);

        response.clicked()
    }

    pub fn button_with(
        &mut self,
        gui: &mut Gui<'_>,
        rect: Rect,
        enabled: bool,
        id_source: impl std::hash::Hash,
        shapes: impl IntoIterator<Item = Shape>,
        tooltip: &str,
    ) -> bool {
        let id = gui.ui().make_persistent_id(id_source);
        let response = gui.ui().interact(
            rect,
            id,
            enabled.then_else(Sense::click() | Sense::hover(), Sense::hover()),
        );
        if response.hovered() && !tooltip.is_empty() {
            response.show_tooltip_text(tooltip);
        }
        let fill = if !enabled {
            gui.style.noninteractive_bg_fill
        } else if response.is_pointer_button_down_on() {
            gui.style.active_bg_fill
        } else if response.hovered() {
            gui.style.hover_bg_fill
        } else {
            gui.style.inactive_bg_fill
        };
        let stroke = gui.style.inactive_bg_stroke;

        gui.painter().rect(
            rect,
            gui.style.small_corner_radius * gui.scale,
            fill,
            stroke,
            StrokeKind::Middle,
        );
        gui.painter().extend(shapes);

        response.clicked()
    }
}

impl<'a> std::fmt::Debug for GraphContext<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphContext")
            .field("func_lib", &"FuncLib")
            .finish()
    }
}
