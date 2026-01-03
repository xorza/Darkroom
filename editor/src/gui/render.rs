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
    pub origin: Pos2,
    pub scale: f32,

    pub node_layout: node_ui::NodeLayout,
    pub node_widths: HashMap<NodeId, f32>,
}

impl<'a> RenderContext<'a> {
    pub fn new(
        ui: &'a mut Ui,
        painter: Painter,
        rect: Rect,
        pan: Vec2,
        scale: f32,
        view_graph: &ViewGraph,
        func_lib: &FuncLib,
    ) -> Self {
        let style = Style::new();
        let origin = rect.min + pan;

        let node_layout = node_ui::NodeLayout::default().scaled(view_graph.zoom);
        let width_ctx = node_ui::NodeWidthContext {
            node_layout: &node_layout,
            style: &style,
            scale: view_graph.zoom,
        };
        let node_widths = node_ui::compute_node_widths(&painter, view_graph, func_lib, &width_ctx);

        Self {
            ui,
            painter,
            rect,
            style,
            origin,
            scale,

            node_layout,
            node_widths,
        }
    }

    pub fn node_width(&self, node_id: NodeId) -> f32 {
        self.node_widths
            .get(&node_id)
            .copied()
            .expect("node width must be precomputed")
    }

    pub fn node_rect(
        &self,
        view_node: &model::ViewNode,
        input_count: usize,
        output_count: usize,
    ) -> Rect {
        node_ui::node_rect_for_graph(
            self.origin,
            view_node,
            input_count,
            output_count,
            self.scale,
            &self.node_layout,
            self.node_width(view_node.id),
        )
    }
}
