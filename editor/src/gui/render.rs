use eframe::egui;
use egui::{Painter, Pos2, Rect, Ui, Vec2};
use graph::graph::{Node, NodeId};
use graph::prelude::FuncLib;
use hashbrown::HashMap;
use std::marker::PhantomData;

use crate::{
    gui::{node_ui, style::Style},
    model::{self, ViewGraph},
};

pub struct RenderContext<'a> {
    pub ui: &'a mut Ui,
    pub painter: Painter,
    pub rect: Rect,
    pub style: Style,
}

impl<'a> RenderContext<'a> {
    pub fn new(ui: &'a mut Ui) -> Self {
        let style = Style::new();
        let rect = ui.available_rect_before_wrap();
        let painter = ui.painter_at(rect);

        Self {
            ui,
            painter,
            rect,
            style,
        }
    }

}
