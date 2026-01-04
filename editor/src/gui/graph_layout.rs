use eframe::egui;
use egui::{Pos2, Rect};
use graph::graph::NodeId;
use graph::prelude::FuncLib;
use hashbrown::HashMap;

use crate::gui::connection_ui::PortKind;
use crate::gui::node_ui::{self, NodeLayout};
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
    pub node_widths: HashMap<NodeId, f32>,
    pub ports: Vec<PortInfo>,
}

impl GraphLayout {
    pub fn build(
        ctx: &RenderContext,
        view_graph: &model::ViewGraph,
        func_lib: &FuncLib,
    ) -> GraphLayout {
        let origin = ctx.rect.min + view_graph.pan;
        let node_layout = node_ui::NodeLayout::default().scaled(view_graph.zoom);
        let width_ctx = node_ui::NodeWidthContext {
            node_layout: &node_layout,
            style: &ctx.style,
            scale: view_graph.zoom,
        };
        let node_widths =
            node_ui::compute_node_widths(&ctx.painter, view_graph, func_lib, &width_ctx);

        let ports = collect_ports(view_graph, func_lib, &node_widths, origin, &node_layout);
        GraphLayout {
            origin,
            scale: view_graph.zoom,
            rect: ctx.rect,
            node_layout,
            node_widths,
            ports,
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

    pub fn hovered_port(
        &self,
        ctx: &RenderContext,
        pointer_pos: Option<Pos2>,
        rect: Rect,
    ) -> Option<PortInfo> {
        pointer_pos
            .filter(|pos| rect.contains(*pos))
            .and_then(|pos| find_port_near(&self.ports, pos, ctx.style.port_activation_radius))
    }

    pub fn pointer_over_node(
        &self,
        pointer_pos: Option<Pos2>,

        view_graph: &model::ViewGraph,
        func_lib: &FuncLib,
    ) -> bool {
        pointer_pos
            .filter(|pos| self.rect.contains(*pos))
            .is_some_and(|pos| {
                view_graph.view_nodes.iter().any(|view_node| {
                    let node = view_graph.graph.by_id(&view_node.id).unwrap();
                    let func = func_lib.by_id(&node.func_id).unwrap();
                    let node_rect =
                        self.node_rect(view_node, func.inputs.len(), func.outputs.len());
                    node_rect.contains(pos)
                })
            })
    }
}

fn collect_ports(
    view_graph: &model::ViewGraph,
    func_lib: &FuncLib,
    node_widths: &HashMap<NodeId, f32>,
    origin: Pos2,
    node_layout: &NodeLayout,
) -> Vec<PortInfo> {
    let mut ports = Vec::new();

    for node_view in view_graph.view_nodes.iter().rev() {
        let node = view_graph.graph.by_id(&node_view.id).unwrap();
        let func = func_lib.by_id(&node.func_id).unwrap();
        let node_width = node_widths.get(&node.id).copied().unwrap();

        for index in 0..func.inputs.len() {
            let center = node_ui::node_input_pos(
                origin,
                node_view,
                index,
                func.inputs.len(),
                node_layout,
                view_graph.zoom,
            );

            ports.push(PortInfo {
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
                origin,
                node_view,
                index,
                node_layout,
                view_graph.zoom,
                node_width,
            );

            ports.push(PortInfo {
                port: PortRef {
                    node_id: node.id,
                    idx: index,
                    kind: PortKind::Output,
                },
                center,
            });
        }
    }

    ports
}

fn find_port_near(ports: &[PortInfo], pos: Pos2, radius: f32) -> Option<PortInfo> {
    assert!(radius.is_finite(), "port activation radius must be finite");
    assert!(radius > 0.0, "port activation radius must be positive");
    let mut best = None;
    let mut best_dist = radius;

    for port in ports {
        let dist = port.center.distance(pos);
        if dist < best_dist {
            best_dist = dist;
            best = Some(port.clone());
        }
    }

    best
}
