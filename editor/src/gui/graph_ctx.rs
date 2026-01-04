use eframe::egui;
use egui::{Painter, Rect, Ui};
use graph::prelude::FuncLib;

use crate::{gui::style::Style, model::ViewGraph};

pub struct GraphContext<'a> {
    pub ui: &'a mut Ui,
    pub painter: Painter,
    pub rect: Rect,
    pub style: Style,

    pub view_graph: &'a mut ViewGraph,
    pub func_lib: &'a FuncLib,
}

impl<'a> GraphContext<'a> {
    pub fn new(ui: &'a mut Ui, view_graph: &'a mut ViewGraph, func_lib: &'a FuncLib) -> Self {
        let style = Style::new();
        let rect = ui.available_rect_before_wrap();
        let painter = ui.painter_at(rect);

        Self {
            ui,
            painter,
            rect,
            style,
            view_graph,
            func_lib,
        }
    }
}
