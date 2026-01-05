use eframe::egui;
use egui::{Painter, Rect, Ui};
use graph::prelude::FuncLib;

use crate::gui::style::Style;

#[derive(Debug)]
pub struct GraphContext<'a> {
    pub ui: &'a mut Ui,
    pub painter: Painter,
    pub rect: Rect,
    pub style: Style,

    pub func_lib: &'a FuncLib,
}

impl<'a> GraphContext<'a> {
    pub fn new(ui: &'a mut Ui, func_lib: &'a FuncLib) -> Self {
        let style = Style::new();
        let rect = ui.available_rect_before_wrap();
        let painter = ui.painter_at(rect);

        Self {
            ui,
            painter,
            rect,
            style,
            func_lib,
        }
    }
}
