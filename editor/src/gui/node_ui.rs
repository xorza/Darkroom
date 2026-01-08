use crate::common::font::ScaledFontId;
use crate::gui::connection_ui::PortKind;
use crate::gui::graph_layout::{GraphLayout, PortInfo, PortRef};
use crate::gui::node_layout::NodeLayout;
use common::BoolExt;
use eframe::egui;
use egui::{
    Align2, Color32, PointerButton, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2,
};
use graph::execution_graph::ExecutedNodeStats;
use graph::graph::{Node, NodeId};
use graph::prelude::{ExecutionStats, FuncBehavior, NodeBehavior};

use crate::gui::const_bind_ui::render_const_bindings;
use crate::gui::{
    Gui, graph_ctx::GraphContext, graph_ui::GraphUiAction, graph_ui::GraphUiInteraction,
};
use crate::model::{ViewGraph, ViewNode};

#[derive(Debug, Clone)]
pub enum PortDragInfo {
    None,
    Hover(PortInfo),
    DragStart(PortInfo),
    DragStop,
}

#[derive(Debug, Default)]
pub struct NodeUi {
    node_ids_to_remove: Vec<NodeId>,
}

#[derive(Debug)]
enum NodeExecutionInfo<'a> {
    MissingInputs,
    Executed(&'a ExecutedNodeStats),
    Cached,
    None,
}

impl NodeUi {
    pub fn render_nodes(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext,
        view_graph: &mut ViewGraph,
        graph_layout: &mut GraphLayout,
        ui_interaction: &mut GraphUiInteraction,
        execution_stats: Option<&ExecutionStats>,
    ) -> PortDragInfo {
        self.node_ids_to_remove.clear();
        let mut drag_port_info: PortDragInfo = PortDragInfo::None;

        for view_node_idx in 0..view_graph.view_nodes.len() {
            let node_id = view_graph.view_nodes[view_node_idx].id;
            let node_layout =
                body_drag(gui, ctx, view_graph, graph_layout, ui_interaction, &node_id);

            let node = view_graph.graph.by_id_mut(&node_id).unwrap();
            let func = ctx.func_lib.by_id(&node.func_id).unwrap();
            let view_node = &view_graph.view_nodes[view_node_idx];

            let is_selected = view_graph.selected_node_id.is_some_and(|id| id == node_id);

            let node_execution_info = node_execution_info(node_id, execution_stats);

            render_body(gui, ctx, node_layout, is_selected, &node_execution_info);
            if render_remove_btn(gui, ctx, ui_interaction, &node_id, node_layout) {
                self.node_ids_to_remove.push(node_id);
            }
            render_cache_btn(gui, ctx, ui_interaction, node_layout, node);
            render_hints(gui, ctx, node_layout, node, func);
            render_const_bindings(ctx, gui, ui_interaction, node_layout, node);
            let node_drag_port_result = render_ports(gui, ctx, node_layout, view_node);
            drag_port_info = drag_port_info.prefer(node_drag_port_result);
            render_port_labels(gui, ctx, node_layout);
        }

        while let Some(node_id) = self.node_ids_to_remove.pop() {
            view_graph.remove_node(&node_id);
        }

        drag_port_info
    }
}

fn body_drag<'a>(
    gui: &mut Gui<'_>,
    _ctx: &mut GraphContext<'_>,
    view_graph: &mut ViewGraph,
    graph_layout: &'a mut GraphLayout,
    ui_interaction: &mut GraphUiInteraction,
    node_id: &NodeId,
) -> &'a NodeLayout {
    let node_layout = graph_layout.node_layouts.by_key_mut(node_id).unwrap();

    let node_body_id = gui.ui().make_persistent_id(("node_body", node_id));
    let body_response = gui.ui().interact(
        node_layout.body_rect,
        node_body_id,
        Sense::click() | Sense::hover() | Sense::drag(),
    );

    let dragged = body_response.dragged_by(PointerButton::Middle)
        || body_response.dragged_by(PointerButton::Primary);

    if dragged || body_response.clicked() {
        ui_interaction
            .actions
            .push((*node_id, GraphUiAction::NodeSelected));
        view_graph.selected_node_id = Some(*node_id);
    }
    if dragged {
        view_graph.view_nodes.by_key_mut(node_id).unwrap().pos +=
            body_response.drag_delta() / gui.scale;

        node_layout.update(_ctx, gui, view_graph, graph_layout.origin);
    }

    node_layout
}

fn render_body(
    gui: &Gui<'_>,
    _ctx: &mut GraphContext<'_>,
    node_layout: &NodeLayout,
    selected: bool,
    node_execution_info: &NodeExecutionInfo<'_>,
) {
    let corner_radius = gui.style.corner_radius * gui.scale;

    let shadow = match *node_execution_info {
        NodeExecutionInfo::MissingInputs => Some(&gui.style.node.missing_inputs_shadow),
        NodeExecutionInfo::Executed(_) => Some(&gui.style.node.executed_shadow),
        NodeExecutionInfo::Cached => Some(&gui.style.node.cached_shadow),
        NodeExecutionInfo::None => None,
    };

    if let Some(shadow) = shadow {
        gui.painter().add(Shape::Rect(
            shadow.as_shape(node_layout.body_rect, corner_radius),
        ));
    }

    gui.painter().rect(
        node_layout.body_rect,
        corner_radius,
        gui.style.noninteractive_bg_fill,
        gui.style.inactive_bg_stroke,
        StrokeKind::Middle,
    );
    if let NodeExecutionInfo::Executed(executed) = *node_execution_info {
        let text_pos = pos2(
            node_layout.body_rect.min.x,
            node_layout.body_rect.max.y + gui.style.small_padding * gui.scale,
        );
        let label = format!("{:.1} ms", executed.elapsed_secs * 1000.0);
        gui.painter().text(
            text_pos,
            Align2::LEFT_TOP,
            label,
            gui.style.sub_font.scaled(gui.scale),
            gui.style.noninteractive_text_color,
        );
    }
    if selected {
        let mut header_rect = node_layout.body_rect;
        header_rect.max.y = header_rect.min.y + node_layout.header_row_height;

        gui.painter().rect(
            header_rect,
            corner_radius,
            gui.style.active_bg_fill,
            Stroke::NONE,
            StrokeKind::Middle,
        );
    }
    let title_pos = node_layout.body_rect.min
        + vec2(
            gui.style.padding * gui.scale,
            (node_layout.header_row_height - node_layout.title_galley.size().y) * 0.5,
        );
    gui.painter().galley(
        title_pos,
        node_layout.title_galley.clone(),
        gui.style.text_color,
    );
}

fn render_cache_btn(
    gui: &mut Gui<'_>,
    ctx: &mut GraphContext,
    ui_interaction: &mut GraphUiInteraction,
    node_layout: &NodeLayout,
    node: &mut Node,
) {
    let enabled = !node.terminal;
    let checked = node.behavior == NodeBehavior::Once;

    if ctx.toggle_button(
        gui,
        node_layout.cache_button_rect,
        enabled,
        checked,
        (node.id, "cache"),
        "cache",
        "",
    ) {
        node.behavior = (node.behavior == NodeBehavior::Once)
            .then_else(NodeBehavior::AsFunction, NodeBehavior::Once);
        ui_interaction
            .actions
            .push((node.id, GraphUiAction::CacheToggled));
    };
}

fn render_hints(
    gui: &mut Gui<'_>,
    _ctx: &mut GraphContext,
    node_layout: &NodeLayout,
    node: &graph::prelude::Node,
    func: &graph::prelude::Func,
) {
    let dot_radius = gui.scale * gui.style.node.status_dot_radius;
    let dot_step = (dot_radius * 2.0) + gui.style.small_padding * gui.scale;

    if node.terminal {
        let center = node_layout.dot_center(0, dot_step);
        gui.painter()
            .circle_filled(center, dot_radius, gui.style.node.status_terminal_color);
        let dot_rect =
            egui::Rect::from_center_size(center, vec2(dot_radius * 2.0, dot_radius * 2.0));
        let dot_id = gui
            .ui()
            .make_persistent_id(("node_status_terminal", node.id));
        let dot_response = gui.ui().interact(dot_rect, dot_id, Sense::hover());
        if dot_response.hovered() {
            dot_response.show_tooltip_text("terminal");
        }
    }
    if node.behavior == NodeBehavior::AsFunction && func.behavior == FuncBehavior::Impure {
        let center = node_layout.dot_center(usize::from(node.terminal), dot_step);
        gui.painter()
            .circle_filled(center, dot_radius, gui.style.node.status_impure_color);
        let dot_rect = Rect::from_center_size(center, vec2(dot_radius * 2.0, dot_radius * 2.0));
        let dot_id = gui.ui().make_persistent_id(("node_status_impure", node.id));
        let dot_response = gui.ui().interact(dot_rect, dot_id, Sense::hover());
        if dot_response.hovered() {
            dot_response.show_tooltip_text("impure");
        }
    }
}

fn render_remove_btn(
    gui: &mut Gui<'_>,
    ctx: &mut GraphContext,
    ui_interaction: &mut GraphUiInteraction,
    node_id: &NodeId,
    node_layout: &NodeLayout,
) -> bool {
    let remove_rect = node_layout.remove_btn_rect;
    let remove_margin = remove_rect.width() * 0.3;
    let a = pos2(
        remove_rect.min.x + remove_margin,
        remove_rect.min.y + remove_margin,
    );
    let b = pos2(
        remove_rect.max.x - remove_margin,
        remove_rect.max.y - remove_margin,
    );
    let c = pos2(
        remove_rect.min.x + remove_margin,
        remove_rect.max.y - remove_margin,
    );
    let d = pos2(
        remove_rect.max.x - remove_margin,
        remove_rect.min.y + remove_margin,
    );
    let remove_color = gui.style.text_color;
    let remove_stroke = Stroke::new(1.4 * gui.scale, remove_color);
    let remove_shapes = [
        Shape::line_segment([a, b], remove_stroke),
        Shape::line_segment([c, d], remove_stroke),
    ];
    let remove = ctx.button_with(
        gui,
        remove_rect,
        true,
        ("node_remove", node_id),
        remove_shapes.iter().cloned(),
        "Remove node",
    );

    if remove {
        ui_interaction
            .actions
            .push((*node_id, GraphUiAction::NodeRemoved));
        return true;
    }

    false
}

fn render_ports(
    gui: &mut Gui<'_>,
    _ctx: &GraphContext,
    node_layout: &NodeLayout,
    view_node: &ViewNode,
) -> PortDragInfo {
    let port_radius = gui.style.node.port_radius * gui.scale;
    let port_rect_size = Vec2::ONE * 2.0 * node_layout.port_activation_radius;

    let input_base = gui.style.node.input_port_color;
    let input_hover = gui.style.node.input_hover_color;
    let output_base = gui.style.node.output_port_color;
    let output_hover = gui.style.node.output_hover_color;

    let mut draw_port = |center: Pos2,
                         kind: PortKind,
                         idx: usize,
                         base_color: Color32,
                         hover_color: Color32|
     -> PortDragInfo {
        let port_rect = egui::Rect::from_center_size(center, port_rect_size);
        let ui = gui.ui();
        let port_id = ui.make_persistent_id(("node_port", kind, view_node.id, idx));
        let response = ui.interact(port_rect, port_id, Sense::drag() | Sense::hover());
        let is_hovered = ui.rect_contains_pointer(port_rect);

        let color = is_hovered.then_else(hover_color, base_color);
        gui.painter().circle_filled(center, port_radius, color);

        let port_info = PortInfo {
            port: PortRef {
                node_id: view_node.id,
                idx,
                kind,
            },
            center,
        };
        if response.drag_started_by(PointerButton::Primary) {
            PortDragInfo::DragStart(port_info)
        } else if response.drag_stopped_by(PointerButton::Primary) {
            PortDragInfo::DragStop
        } else if is_hovered {
            PortDragInfo::Hover(port_info)
        } else {
            PortDragInfo::None
        }
    };

    let mut port_drag_info: PortDragInfo = PortDragInfo::None;

    for input_idx in 0..node_layout.input_galleys.len() {
        let center = node_layout.input_center(input_idx);
        let drag_info = draw_port(center, PortKind::Input, input_idx, input_base, input_hover);
        port_drag_info = port_drag_info.prefer(drag_info);
    }

    for output_idx in 0..node_layout.output_galleys.len() {
        let center = node_layout.output_center(output_idx);
        let drag_info = draw_port(
            center,
            PortKind::Output,
            output_idx,
            output_base,
            output_hover,
        );
        port_drag_info = port_drag_info.prefer(drag_info);
    }

    port_drag_info
}

fn render_port_labels(gui: &Gui<'_>, _ctx: &mut GraphContext, node_layout: &NodeLayout) {
    let padding = gui.style.node.port_label_side_padding * gui.scale;

    for (input_idx, galley) in node_layout.input_galleys.iter().enumerate() {
        let text_pos = node_layout.input_center(input_idx) + vec2(padding, -galley.size().y * 0.5);
        gui.painter()
            .galley(text_pos, galley.clone(), gui.style.text_color);
    }

    for (output_idx, galley) in node_layout.output_galleys.iter().enumerate() {
        let text_pos = node_layout.output_center(output_idx)
            + vec2(-padding - galley.size().x, -galley.size().y * 0.5);
        gui.painter()
            .galley(text_pos, galley.clone(), gui.style.text_color);
    }
}

fn node_execution_info<'a>(
    node_id: NodeId,
    execution_stats: Option<&'a ExecutionStats>,
) -> NodeExecutionInfo<'a> {
    let Some(execution_stats) = execution_stats else {
        return NodeExecutionInfo::None;
    };

    if execution_stats.nodes_with_missing_inputs.contains(&node_id) {
        return NodeExecutionInfo::MissingInputs;
    }

    if let Some(executed) = execution_stats
        .executed_nodes
        .iter()
        .find(|stats| stats.node_id == node_id)
    {
        return NodeExecutionInfo::Executed(executed);
    }

    if execution_stats.cached_nodes.contains(&node_id) {
        return NodeExecutionInfo::Cached;
    }

    NodeExecutionInfo::None
}

impl PortDragInfo {
    fn prio(&self) -> u32 {
        match self {
            PortDragInfo::None => 0,
            PortDragInfo::Hover(_) => 5,
            PortDragInfo::DragStart(_) => 8,
            PortDragInfo::DragStop => 10,
        }
    }

    fn prefer(self, other: Self) -> Self {
        (other.prio() > self.prio()).then_else(other, self)
    }
}
