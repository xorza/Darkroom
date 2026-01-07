use eframe::egui;
use egui::{Painter, Rect, Ui};
use graph::prelude::FuncLib;

use crate::gui::style::Style;

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
}
