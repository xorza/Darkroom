use crate::common::button::Button;
use crate::common::drag_state::{DragResult, DragState};
use crate::common::id_salt::{NodeIds, PortIds};
use crate::common::primitives::draw_circle_with_gradient_shadow;
use crate::gui::Gui;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_ui::PortKind;
use crate::gui::const_bind_ui::ConstBindUi;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_layout::{GraphLayout, PortInfo, PortRef};
use crate::gui::graph_ui_interaction::GraphUiInteraction;
use crate::gui::node_layout::NodeLayout;
use crate::model::graph_ui_action::GraphUiAction;
use common::BoolExt;
use egui::epaint::CornerRadiusF32;
use egui::{
    Align2, Color32, PointerButton, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2,
};
use scenarium::execution_stats::{ExecutedNodeStats, NodeError};
use scenarium::graph::{Node, NodeId};
use scenarium::prelude::{ExecutionStats, Func, FuncBehavior, NodeBehavior};

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone)]
pub enum PortInteractCommand {
    None,
    Hover(PortInfo),
    DragStart(PortInfo),
    DragStop,
    Click(PortInfo),
}

impl PortInteractCommand {
    fn priority(&self) -> u8 {
        match self {
            Self::None => 0,
            Self::Hover(_) => 5,
            Self::DragStart(_) => 8,
            Self::DragStop => 10,
            Self::Click(_) => 15,
        }
    }

    fn prefer(self, other: Self) -> Self {
        if other.priority() > self.priority() {
            other
        } else {
            self
        }
    }
}

#[derive(Debug)]
pub(crate) enum NodeExecutionInfo<'a> {
    Errored(&'a NodeError),
    MissingInputs,
    Executed(&'a ExecutedNodeStats),
    Cached,
    None,
}

impl<'a> NodeExecutionInfo<'a> {
    pub(crate) fn from_stats(stats: Option<&'a ExecutionStats>, node_id: NodeId) -> Self {
        let Some(stats) = stats else {
            return Self::None;
        };

        if let Some(err) = stats.node_errors.iter().find(|e| e.node_id == node_id) {
            return Self::Errored(err);
        }

        if stats.missing_inputs.iter().any(|p| p.target_id == node_id) {
            return Self::MissingInputs;
        }

        if let Some(executed) = stats.executed_nodes.iter().find(|s| s.node_id == node_id) {
            return Self::Executed(executed);
        }

        if stats.cached_nodes.contains(&node_id) {
            return Self::Cached;
        }

        Self::None
    }

    fn shadow<'b>(&self, gui: &'b Gui<'_>) -> Option<&'b egui::Shadow> {
        match self {
            Self::Errored(_) => Some(&gui.style.node.errored_shadow),
            Self::MissingInputs => Some(&gui.style.node.missing_inputs_shadow),
            Self::Executed(_) => Some(&gui.style.node.executed_shadow),
            Self::Cached => Some(&gui.style.node.cached_shadow),
            Self::None => None,
        }
    }
}

// ============================================================================
// NodeUi
// ============================================================================

#[derive(Debug, Default)]
pub(crate) struct NodeUi {
    node_ids_to_remove: Vec<NodeId>,
    node_ids_hit_breaker: Vec<NodeId>,
    pub(crate) const_bind_ui: ConstBindUi,
}

impl NodeUi {
    pub(crate) fn broke_node_iter(&self) -> impl Iterator<Item = &NodeId> {
        self.node_ids_hit_breaker.iter()
    }

    pub fn render_nodes(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext,
        graph_layout: &mut GraphLayout,
        interaction: &mut GraphUiInteraction,
        breaker: Option<&ConnectionBreaker>,
    ) -> PortInteractCommand {
        self.node_ids_to_remove.clear();
        self.node_ids_hit_breaker.clear();

        let mut port_cmd = PortInteractCommand::None;
        let mut const_bind_frame = self.const_bind_ui.start();

        for view_node_idx in 0..ctx.view_graph.view_nodes.len() {
            let node_id = ctx.view_graph.view_nodes[view_node_idx].id;
            let layout = Self::handle_node_drag(gui, ctx, graph_layout, interaction, &node_id);

            let node = ctx.view_graph.graph.by_id_mut(&node_id).unwrap();
            let func = ctx.func_lib.by_id(&node.func_id).unwrap();

            const_bind_frame.render(gui, interaction, layout, node, func, breaker);

            if !gui.ui().is_rect_visible(layout.body_rect) {
                continue;
            }

            let is_selected = ctx.view_graph.selected_node_id == Some(node_id);
            let exec_info = NodeExecutionInfo::from_stats(ctx.execution_stats, node_id);

            if render_body(gui, layout, is_selected, &exec_info, breaker) {
                self.node_ids_hit_breaker.push(node_id);
            }

            if render_remove_btn(gui, layout) {
                self.node_ids_to_remove.push(node_id);
            }

            render_status_hints(gui, layout, node_id, node.behavior, func);
            render_cache_btn(gui, interaction, layout, node);

            let missing_inputs = get_missing_input_ports(ctx.execution_stats, node_id);
            let node_port_cmd = render_ports(gui, layout, node, func, &missing_inputs);
            port_cmd = port_cmd.prefer(node_port_cmd);

            render_port_labels(gui, layout);
        }

        const_bind_frame.finish();
        for node_id in self.node_ids_to_remove.drain(..) {
            let action = ctx.view_graph.removal_action(&node_id);
            ctx.view_graph.remove_node(&node_id);
            interaction.add_action(action);
        }

        port_cmd
    }

    fn handle_node_drag<'a>(
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        graph_layout: &'a mut GraphLayout,
        interaction: &mut GraphUiInteraction,
        node_id: &NodeId,
    ) -> &'a NodeLayout {
        let layout = graph_layout.node_layouts.by_key_mut(node_id).unwrap();

        let body_id = gui.ui().make_persistent_id(NodeIds::body(*node_id));
        let response = gui.ui().interact(
            layout.body_rect,
            body_id,
            Sense::click() | Sense::hover() | Sense::drag(),
        );

        let dragged = response.dragged_by(PointerButton::Middle)
            || response.dragged_by(PointerButton::Primary);

        // Handle selection
        if (dragged || response.clicked()) && ctx.view_graph.selected_node_id != Some(*node_id) {
            let before = ctx.view_graph.selected_node_id;
            ctx.view_graph.selected_node_id = Some(*node_id);
            interaction.add_action(GraphUiAction::NodeSelected {
                before,
                after: Some(*node_id),
            });
        }

        // Handle drag with DragState
        let drag_id = gui.ui().make_persistent_id(NodeIds::drag_start(*node_id));
        let drag_state = DragState::<Pos2>::new(drag_id);
        let current_pos = ctx.view_graph.view_nodes.by_key(node_id).unwrap().pos;

        match drag_state.update(gui.ui(), &response, current_pos) {
            DragResult::Started | DragResult::Dragging => {
                if dragged {
                    ctx.view_graph.view_nodes.by_key_mut(node_id).unwrap().pos +=
                        response.drag_delta() / gui.scale();
                    layout.update(ctx, gui, graph_layout.origin);
                }
            }
            DragResult::Stopped { start_value } => {
                let end_pos = ctx.view_graph.view_nodes.by_key(node_id).unwrap().pos;
                interaction.add_action(GraphUiAction::NodeMoved {
                    node_id: *node_id,
                    before: start_value,
                    after: end_pos,
                });
            }
            DragResult::Idle => {}
        }

        layout
    }
}

// ============================================================================
// Body rendering
// ============================================================================

fn render_body(
    gui: &Gui<'_>,
    layout: &NodeLayout,
    selected: bool,
    exec_info: &NodeExecutionInfo<'_>,
    breaker: Option<&ConnectionBreaker>,
) -> bool {
    let corner_radius = gui.style.corner_radius;
    let breaker_hit = breaker.is_some_and(|b| b.intersects_rect(layout.body_rect));

    // Base shadow
    gui.painter().add(Shape::Rect(
        gui.style
            .node
            .shadow
            .as_shape(layout.body_rect, corner_radius),
    ));

    // Execution state shadow
    if let Some(shadow) = exec_info.shadow(gui) {
        gui.painter().add(Shape::Rect(
            shadow.as_shape(layout.body_rect, corner_radius),
        ));
    }

    // Body rectangle
    gui.painter().rect(
        layout.body_rect,
        corner_radius,
        gui.style.noninteractive_bg_fill,
        gui.style.inactive_bg_stroke,
        StrokeKind::Middle,
    );

    // Execution time label
    if let NodeExecutionInfo::Executed(stats) = exec_info {
        let text_pos = pos2(
            layout.body_rect.min.x,
            layout.body_rect.max.y + gui.style.small_padding,
        );
        gui.painter().text(
            text_pos,
            Align2::LEFT_TOP,
            format!("{:.1} ms", stats.elapsed_secs * 1000.0),
            gui.style.sub_font.clone(),
            gui.style.noninteractive_text_color,
        );
    }

    // Header highlight (selected or breaker hit)
    if selected || breaker_hit {
        let header_rect = Rect::from_min_max(
            layout.body_rect.min,
            pos2(
                layout.body_rect.max.x,
                layout.body_rect.min.y + layout.header_row_height,
            ),
        );
        let fill = breaker_hit.then_else(gui.style.connections.broke_clr, gui.style.active_bg_fill);

        gui.painter().rect(
            header_rect,
            CornerRadiusF32 {
                nw: corner_radius,
                ne: corner_radius,
                sw: 0.0,
                se: 0.0,
            },
            fill,
            Stroke::NONE,
            StrokeKind::Middle,
        );
    }

    // Title
    let title_pos = layout.body_rect.min
        + vec2(
            gui.style.padding,
            (layout.header_row_height - layout.title_galley.size().y) * 0.5,
        );
    let title_color = breaker_hit.then_else(gui.style.dark_text_color, gui.style.text_color);
    gui.painter().galley_with_override_text_color(
        title_pos,
        layout.title_galley.clone(),
        title_color,
    );

    breaker_hit
}

// ============================================================================
// UI controls
// ============================================================================

fn render_cache_btn(
    gui: &mut Gui<'_>,
    interaction: &mut GraphUiInteraction,
    layout: &NodeLayout,
    node: &mut Node,
) {
    if !layout.has_cache_btn {
        return;
    }

    let mut checked = node.behavior == NodeBehavior::Once;
    let response = Button::default()
        .toggle(&mut checked)
        .text("cache")
        .rect(layout.cache_button_rect)
        .show(gui);

    if response.clicked() {
        let before = node.behavior;
        node.behavior.toggle();
        interaction.add_action(GraphUiAction::CacheToggled {
            node_id: node.id,
            before,
            after: node.behavior,
        });
    }
}

fn render_remove_btn(gui: &mut Gui<'_>, layout: &NodeLayout) -> bool {
    let rect = layout.remove_btn_rect;
    let margin = rect.width() * 0.3;

    // X shape corners
    let tl = pos2(rect.min.x + margin, rect.min.y + margin);
    let br = pos2(rect.max.x - margin, rect.max.y - margin);
    let bl = pos2(rect.min.x + margin, rect.max.y - margin);
    let tr = pos2(rect.max.x - margin, rect.min.y + margin);

    let stroke = Stroke::new(1.4 * gui.scale(), gui.style.text_color);
    let shapes = [
        Shape::line_segment([tl, br], stroke),
        Shape::line_segment([bl, tr], stroke),
    ];

    Button::default()
        .tooltip("Remove node")
        .rect(rect)
        .shapes(shapes)
        .show(gui)
        .clicked()
}

fn render_status_hints(
    gui: &mut Gui<'_>,
    layout: &NodeLayout,
    node_id: NodeId,
    behavior: NodeBehavior,
    func: &Func,
) {
    // Show impure indicator for non-cached impure functions with outputs
    let show_impure = behavior == NodeBehavior::AsFunction
        && func.behavior == FuncBehavior::Impure
        && !func.outputs.is_empty();

    if !show_impure {
        return;
    }

    let dot_radius = gui.style.node.status_dot_radius;
    let dot_step = dot_radius * 2.0 + gui.style.small_padding;
    let center = layout.dot_center(0, dot_step);

    gui.painter()
        .circle_filled(center, dot_radius, gui.style.node.status_impure_color);

    // Tooltip on hover
    let dot_rect = Rect::from_center_size(center, vec2(dot_radius * 2.0, dot_radius * 2.0));
    let dot_id = gui.ui().make_persistent_id(NodeIds::status_impure(node_id));
    let response = gui.ui().interact(dot_rect, dot_id, Sense::hover());

    if response.hovered() {
        response.show_tooltip_text("impure");
    }
}

// ============================================================================
// Port rendering
// ============================================================================

fn get_missing_input_ports(stats: Option<&ExecutionStats>, node_id: NodeId) -> Vec<usize> {
    stats
        .map(|s| {
            s.missing_inputs
                .iter()
                .filter(|p| p.target_id == node_id)
                .map(|p| p.port_idx)
                .collect()
        })
        .unwrap_or_default()
}

fn render_ports(
    gui: &mut Gui<'_>,
    layout: &NodeLayout,
    node: &Node,
    func: &Func,
    missing_inputs: &[usize],
) -> PortInteractCommand {
    let port_radius = gui.style.node.port_radius;
    let port_rect_size = Vec2::splat(layout.port_activation_radius * 2.0);
    let missing_shadow_color = gui.style.node.missing_inputs_shadow.color;
    let missing_shadow_spread = gui.style.node.missing_inputs_shadow.spread as f32;

    let mut result = PortInteractCommand::None;

    // Helper closure for drawing a single port
    let draw_port = |gui: &mut Gui<'_>,
                     center: Pos2,
                     kind: PortKind,
                     idx: usize,
                     show_missing_shadow: bool|
     -> PortInteractCommand {
        let port_rect = Rect::from_center_size(center, port_rect_size);
        let port_id = gui
            .ui()
            .make_persistent_id(PortIds::port(node.id, kind, idx));
        let response = gui.ui().interact(
            port_rect,
            port_id,
            Sense::drag() | Sense::hover() | Sense::click(),
        );
        let hovered = gui.ui().rect_contains_pointer(port_rect);

        if show_missing_shadow {
            draw_circle_with_gradient_shadow(
                gui.painter(),
                center,
                port_radius,
                port_radius + missing_shadow_spread * 2.0,
                missing_shadow_color,
            );
        }

        let color = gui.style.node.port_colors(kind).select(hovered);
        gui.painter().circle_filled(center, port_radius, color);

        let port_info = PortInfo {
            port: PortRef {
                node_id: node.id,
                port_idx: idx,
                kind,
            },
            center,
        };

        if response.drag_started_by(PointerButton::Primary) {
            PortInteractCommand::DragStart(port_info)
        } else if response.drag_stopped_by(PointerButton::Primary) {
            PortInteractCommand::DragStop
        } else if !response.dragged() && response.clicked_by(PointerButton::Primary) {
            PortInteractCommand::Click(port_info)
        } else if hovered {
            PortInteractCommand::Hover(port_info)
        } else {
            PortInteractCommand::None
        }
    };

    // Trigger port (for terminal nodes)
    if func.terminal {
        let cmd = draw_port(gui, layout.trigger_center(), PortKind::Trigger, 0, false);
        result = result.prefer(cmd);
    }

    // Input ports
    for idx in 0..layout.input_galleys.len() {
        let cmd = draw_port(
            gui,
            layout.input_center(idx),
            PortKind::Input,
            idx,
            missing_inputs.contains(&idx),
        );
        result = result.prefer(cmd);
    }

    // Output ports
    for idx in 0..layout.output_galleys.len() {
        let cmd = draw_port(gui, layout.output_center(idx), PortKind::Output, idx, false);
        result = result.prefer(cmd);
    }

    // Event ports
    for idx in 0..layout.event_galleys.len() {
        let cmd = draw_port(gui, layout.event_center(idx), PortKind::Event, idx, false);
        result = result.prefer(cmd);
    }

    result
}

fn render_port_labels(gui: &Gui<'_>, layout: &NodeLayout) {
    let padding = gui.style.node.port_label_side_padding;

    for (idx, galley) in layout.input_galleys.iter().enumerate() {
        let pos = layout.input_center(idx) + vec2(padding, -galley.size().y * 0.5);
        gui.painter()
            .galley(pos, galley.clone(), gui.style.text_color);
    }

    for (idx, galley) in layout.output_galleys.iter().enumerate() {
        let pos =
            layout.output_center(idx) + vec2(-padding - galley.size().x, -galley.size().y * 0.5);
        gui.painter()
            .galley(pos, galley.clone(), gui.style.text_color);
    }

    for (idx, galley) in layout.event_galleys.iter().enumerate() {
        let pos =
            layout.event_center(idx) + vec2(-padding - galley.size().x, -galley.size().y * 0.5);
        gui.painter()
            .galley(pos, galley.clone(), gui.style.text_color);
    }
}
