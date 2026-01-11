use std::any;

use crate::common::button::Button;
use crate::common::toggle_button::ToggleButton;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_ui::PortKind;
use crate::gui::graph_layout::{GraphLayout, PortInfo, PortRef};
use crate::gui::graph_ui_interaction::GraphUiAction;
use crate::gui::node_layout::NodeLayout;
use common::BoolExt;
use eframe::egui;
use egui::epaint::CornerRadiusF32;
use egui::{
    Align2, Color32, CornerRadius, PointerButton, Pos2, Rect, Sense, Shape, Stroke, StrokeKind,
    Vec2, pos2, vec2,
};
use graph::execution_graph::ExecutedNodeStats;
use graph::graph::{Node, NodeId};
use graph::prelude::{ExecutionStats, FuncBehavior, NodeBehavior};

use crate::gui::const_bind_ui::ConstBindUi;
use crate::gui::{Gui, graph_ctx::GraphContext, graph_ui_interaction::GraphUiInteraction};

#[derive(Debug, Clone)]
pub enum PortDragInfo {
    None,
    Hover(PortInfo),
    DragStart(PortInfo),
    DragStop,
}

#[derive(Debug, Default)]
pub(crate) struct NodeUi {
    node_ids_to_remove: Vec<NodeId>,
    pub(crate) node_ids_hit_breaker: Vec<NodeId>,
    pub(crate) const_bind_ui: ConstBindUi,
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
        graph_layout: &mut GraphLayout,
        ui_interaction: &mut GraphUiInteraction,
        breaker: Option<&ConnectionBreaker>,
    ) -> PortDragInfo {
        self.node_ids_to_remove.clear();
        self.node_ids_hit_breaker.clear();

        let mut drag_port_info: PortDragInfo = PortDragInfo::None;
        let mut const_bind_frame = self.const_bind_ui.start();

        let view_node_count = ctx.view_graph.view_nodes.len();
        for view_node_idx in 0..view_node_count {
            let node_id = ctx.view_graph.view_nodes[view_node_idx].id;

            let node_layout = body_drag(gui, ctx, graph_layout, ui_interaction, &node_id);

            let node = ctx.view_graph.graph.by_id_mut(&node_id).unwrap();
            let func = ctx.func_lib.by_id(&node.func_id).unwrap();

            let is_selected = ctx
                .view_graph
                .selected_node_id
                .is_some_and(|id| id == node_id);

            let node_execution_info = node_execution_info(ctx.execution_stats, node_id);
            if render_body(gui, node_layout, is_selected, &node_execution_info, breaker) {
                self.node_ids_hit_breaker.push(node_id);
            }
            if render_remove_btn(gui, &node_id, node_layout) {
                self.node_ids_to_remove.push(node_id);
            }
            render_hints(
                gui,
                node_layout,
                node_id,
                node.terminal,
                node.behavior,
                func,
            );
            render_cache_btn(gui, ui_interaction, node_layout, node);
            const_bind_frame.render(gui, ui_interaction, node_layout, node, breaker);

            let node_drag_port_result = render_ports(gui, node_layout, node_id);
            drag_port_info = drag_port_info.prefer(node_drag_port_result);
            render_port_labels(gui, node_layout);
        }

        while let Some(node_id) = self.node_ids_to_remove.pop() {
            let (view_node, node, incoming) = ctx.view_graph.removal_payload(&node_id);
            ctx.view_graph.remove_node(&node_id);
            ui_interaction.add_action(GraphUiAction::NodeRemoved {
                view_node,
                node,
                incoming,
            });
        }

        drag_port_info
    }
}

fn body_drag<'a>(
    gui: &mut Gui<'_>,
    ctx: &mut GraphContext<'_>,
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

    let drag_start_id = gui.ui().make_persistent_id(("node_drag_start", node_id));
    if body_response.drag_started() {
        let start_pos = ctx
            .view_graph
            .view_nodes
            .by_key(node_id)
            .expect("node view must exist to start drag")
            .pos;
        gui.ui()
            .data_mut(|data| data.insert_temp(drag_start_id, start_pos));
    }

    if (dragged || body_response.clicked()) && ctx.view_graph.selected_node_id != Some(*node_id) {
        let before = ctx.view_graph.selected_node_id;
        ui_interaction.add_action(GraphUiAction::NodeSelected {
            before,
            after: Some(*node_id),
        });

        ctx.view_graph.selected_node_id = Some(*node_id);
    }
    if dragged {
        ctx.view_graph.view_nodes.by_key_mut(node_id).unwrap().pos +=
            body_response.drag_delta() / gui.scale;

        node_layout.update(ctx, gui, graph_layout.origin);
    }

    if body_response.drag_stopped() {
        let start_pos = gui
            .ui()
            .data_mut(|data| data.remove_temp::<Pos2>(drag_start_id))
            .expect("node drag must have a start position");
        let end_pos = ctx
            .view_graph
            .view_nodes
            .by_key(node_id)
            .expect("node view must exist after drag")
            .pos;
        ui_interaction.add_action(GraphUiAction::NodeMoved {
            node_id: *node_id,
            before: start_pos,
            after: end_pos,
        });
    }

    node_layout
}

fn render_body(
    gui: &Gui<'_>,
    node_layout: &NodeLayout,
    selected: bool,
    node_execution_info: &NodeExecutionInfo<'_>,
    breaker: Option<&ConnectionBreaker>,
) -> bool {
    let corner_radius = gui.style.corner_radius;
    let breaker_hit = breaker.is_some_and(|breaker| breaker.intersects_rect(node_layout.body_rect));

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
            node_layout.body_rect.max.y + gui.style.small_padding,
        );
        let label = format!("{:.1} ms", executed.elapsed_secs * 1000.0);
        gui.painter().text(
            text_pos,
            Align2::LEFT_TOP,
            label,
            gui.style.sub_font.clone(),
            gui.style.noninteractive_text_color,
        );
    }
    if selected || breaker_hit {
        let mut header_rect = node_layout.body_rect;
        header_rect.max.y = header_rect.min.y + node_layout.header_row_height;
        let header_fill =
            breaker_hit.then_else(gui.style.connections.broke_clr, gui.style.active_bg_fill);

        gui.painter().rect(
            header_rect,
            CornerRadiusF32 {
                nw: corner_radius,
                ne: corner_radius,
                sw: 0.0,
                se: 0.0,
            },
            header_fill,
            Stroke::NONE,
            StrokeKind::Middle,
        );
    }
    let title_pos = node_layout.body_rect.min
        + vec2(
            gui.style.padding,
            (node_layout.header_row_height - node_layout.title_galley.size().y) * 0.5,
        );
    gui.painter().galley_with_override_text_color(
        title_pos,
        node_layout.title_galley.clone(),
        breaker_hit.then_else(gui.style.dark_text_color, gui.style.text_color),
    );

    breaker_hit
}

fn render_cache_btn(
    gui: &mut Gui<'_>,
    ui_interaction: &mut GraphUiInteraction,
    node_layout: &NodeLayout,
    node: &mut Node,
) {
    let enabled = !node.terminal;
    let checked = node.behavior == NodeBehavior::Once;

    let response = ToggleButton::new(gui.ui().make_persistent_id((node.id, "cache")), "cache")
        .enabled(enabled)
        .checked(checked)
        .show(gui, node_layout.cache_button_rect);

    if response.clicked() {
        let before = node.behavior;
        node.behavior.toggle();
        ui_interaction.add_action(GraphUiAction::CacheToggled {
            node_id: node.id,
            before,
            after: node.behavior,
        });
    }
}

fn render_hints(
    gui: &mut Gui<'_>,
    node_layout: &NodeLayout,
    node_id: NodeId,
    is_terminal: bool,
    node_behavior: NodeBehavior,
    func: &graph::prelude::Func,
) {
    let dot_radius = gui.style.node.status_dot_radius;
    let dot_step = (dot_radius * 2.0) + gui.style.small_padding;

    if is_terminal {
        let center = node_layout.dot_center(0, dot_step);
        gui.painter()
            .circle_filled(center, dot_radius, gui.style.node.status_terminal_color);
        let dot_rect =
            egui::Rect::from_center_size(center, vec2(dot_radius * 2.0, dot_radius * 2.0));
        let dot_id = gui
            .ui()
            .make_persistent_id(("node_status_terminal", node_id));
        let dot_response = gui.ui().interact(dot_rect, dot_id, Sense::hover());
        if dot_response.hovered() {
            dot_response.show_tooltip_text("terminal");
        }
    }
    if node_behavior == NodeBehavior::AsFunction && func.behavior == FuncBehavior::Impure {
        let center = node_layout.dot_center(usize::from(is_terminal), dot_step);
        gui.painter()
            .circle_filled(center, dot_radius, gui.style.node.status_impure_color);
        let dot_rect = Rect::from_center_size(center, vec2(dot_radius * 2.0, dot_radius * 2.0));
        let dot_id = gui.ui().make_persistent_id(("node_status_impure", node_id));
        let dot_response = gui.ui().interact(dot_rect, dot_id, Sense::hover());
        if dot_response.hovered() {
            dot_response.show_tooltip_text("impure");
        }
    }
}

fn render_remove_btn(gui: &mut Gui<'_>, node_id: &NodeId, node_layout: &NodeLayout) -> bool {
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
    let remove = Button::new(gui.ui().make_persistent_id(("node_remove", node_id)))
        .enabled(true)
        .tooltip("Remove node")
        .show(gui, remove_rect, remove_shapes)
        .clicked();

    if remove {
        return true;
    }

    false
}

fn render_ports(gui: &mut Gui<'_>, node_layout: &NodeLayout, node_id: NodeId) -> PortDragInfo {
    let port_radius = gui.style.node.port_radius;
    let port_rect_size = Vec2::ONE * 2.0 * node_layout.port_activation_radius;

    let input_base = gui.style.node.input_port_color;
    let input_hover = gui.style.node.input_hover_color;
    let output_base = gui.style.node.output_port_color;
    let output_hover = gui.style.node.output_hover_color;

    let trigger_base = gui.style.node.input_port_color;
    let trigger_hover = gui.style.node.input_hover_color;
    let _event_base = gui.style.node.output_port_color;
    let _event_hover = gui.style.node.output_hover_color;

    let mut draw_port = |center: Pos2,
                         kind: PortKind,
                         idx: usize,
                         base_color: Color32,
                         hover_color: Color32|
     -> PortDragInfo {
        let port_rect = egui::Rect::from_center_size(center, port_rect_size);
        let ui = gui.ui();
        let port_id = ui.make_persistent_id(("node_port", kind, node_id, idx));
        let response = ui.interact(port_rect, port_id, Sense::drag() | Sense::hover());
        let is_hovered = ui.rect_contains_pointer(port_rect);

        let color = is_hovered.then_else(hover_color, base_color);
        gui.painter().circle_filled(center, port_radius, color);

        let port_info = PortInfo {
            port: PortRef {
                node_id,
                port_idx: idx,
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

    {
        // event trigger
        let center = node_layout.body_rect.min;
        port_drag_info = draw_port(center, PortKind::Trigger, 0, trigger_base, trigger_hover)
            .prefer(port_drag_info);
    }

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

fn render_port_labels(gui: &Gui<'_>, node_layout: &NodeLayout) {
    let padding = gui.style.node.port_label_side_padding;

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
    execution_stats: Option<&'a ExecutionStats>,
    node_id: NodeId,
) -> NodeExecutionInfo<'a> {
    let Some(execution_stats) = execution_stats else {
        return NodeExecutionInfo::None;
    };

    if execution_stats
        .missing_inputs
        .iter()
        .any(|port_address| port_address.target_id == node_id)
    {
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
