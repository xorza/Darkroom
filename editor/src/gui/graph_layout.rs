use egui::{Pos2, Rect};
use graph::graph::NodeId;
use graph::prelude::FuncLib;
use hashbrown::HashMap;

use crate::gui::connection_ui::PortKind;
use crate::gui::node_ui;
use crate::gui::render::RenderContext;
use crate::model;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRef {
    pub node_id: NodeId,
    pub idx: usize,
    pub kind: PortKind,
}

#[derive(Debug, Clone)]
pub struct PortInfo {
    pub port: PortRef,
    pub center: Pos2,
}

#[derive(Debug)]
pub struct GraphLayout {
    pub origin: Pos2,
    pub scale: f32,
    pub rect: Rect,
    pub node_layout: node_ui::NodeLayout,
    pub node_rects: HashMap<NodeId, Rect>,
    pub ports: Vec<PortInfo>,
}

impl Default for GraphLayout {
    fn default() -> Self {
        Self {
            origin: Pos2::ZERO,
            scale: 1.0,
            rect: Rect::NOTHING,
            node_layout: node_ui::NodeLayout::default(),
            node_rects: HashMap::new(),
            ports: Vec::new(),
        }
    }
}

impl GraphLayout {
    pub fn update(
        &mut self,
        ctx: &RenderContext,
        view_graph: &model::ViewGraph,
        func_lib: &FuncLib,
    ) {
        self.origin = ctx.rect.min + view_graph.pan;
        self.scale = view_graph.zoom;
        self.rect = ctx.rect;
        self.node_layout = node_ui::NodeLayout::default().scaled(view_graph.zoom);

        node_ui::compute_node_rects(
            ctx,
            view_graph,
            func_lib,
            &self.node_layout,
            self.origin,
            self.scale,
            &mut self.node_rects,
        );
        self.collect_ports(view_graph, func_lib);
    }

    pub fn node_width(&self, node_id: &NodeId) -> f32 {
        self.node_rects
            .get(node_id)
            .copied()
            .expect("node width must be precomputed for view node")
            .width()
    }

    fn node_rect(&self, node_id: &NodeId) -> Rect {
        *self.node_rects.get(node_id).unwrap()
    }

    pub fn hovered_port(&self, pointer_pos: Pos2, port_activation_radius: f32) -> Option<PortInfo> {
        if self.ports.is_empty() {
            return None;
        }
        let mut best = None;
        let mut best_dist = port_activation_radius;

        for port in &self.ports {
            let dist = port.center.distance(pointer_pos);
            if dist < best_dist {
                best_dist = dist;
                best = Some(port.clone());
            }
        }

        best
    }

    pub fn pointer_over_node(&self, pointer_pos: Pos2) -> bool {
        self.node_rects
            .iter()
            .any(|(_, rect)| rect.contains(pointer_pos))
    }

    fn collect_ports(&mut self, view_graph: &model::ViewGraph, func_lib: &FuncLib) {
        self.ports.clear();
        self.ports
            .reserve(Self::port_capacity(view_graph, func_lib));

        for node_view in view_graph.view_nodes.iter().rev() {
            let node = view_graph.graph.by_id(&node_view.id).unwrap();
            let func = func_lib.by_id(&node.func_id).unwrap();
            let node_width = self.node_width(&node.id);

            for index in 0..func.inputs.len() {
                let center = node_ui::node_input_pos(
                    self.origin,
                    node_view,
                    index,
                    func.inputs.len(),
                    &self.node_layout,
                    self.scale,
                );

                self.ports.push(PortInfo {
                    port: PortRef {
                        node_id: node.id,
                        idx: index,
                        kind: PortKind::Input,
                    },
                    center,
                });
            }
            for index in 0..func.outputs.len() {
                let center = node_ui::node_output_pos(
                    self.origin,
                    node_view,
                    index,
                    &self.node_layout,
                    self.scale,
                    node_width,
                );

                self.ports.push(PortInfo {
                    port: PortRef {
                        node_id: node.id,
                        idx: index,
                        kind: PortKind::Output,
                    },
                    center,
                });
            }
        }
    }

    fn port_capacity(view_graph: &model::ViewGraph, func_lib: &FuncLib) -> usize {
        let mut total = 0;
        for node_view in view_graph.view_nodes.iter() {
            let node = view_graph.graph.by_id(&node_view.id).unwrap();
            let func = func_lib.by_id(&node.func_id).unwrap();
            total += func.inputs.len() + func.outputs.len();
        }
        total
    }
}
