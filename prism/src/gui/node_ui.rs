use crate::common::StableId;
use crate::common::primitives::draw_circle_with_gradient_shadow;
use crate::gui::Gui;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_ui::PortKind;
use crate::gui::const_bind_ui::ConstBindUi;
use crate::gui::frame_output::FrameOutput;
use crate::gui::gesture::Gesture;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_layout::{GraphLayout, PortInfo, PortRef};
use crate::gui::node_layout::{NodeGalleys, NodeLayout};
use crate::gui::widgets::button::Button;
use crate::model::execution_info::NodeExecutionInfo;
use crate::model::graph_ui_action::GraphUiAction;
use common::BoolExt;
use egui::epaint::CornerRadiusF32;
use egui::{
    Align2, PointerButton, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2,
};
use scenarium::graph::{Node, NodeId};
use scenarium::prelude::{ExecutionStats, Func, FuncBehavior, NodeBehavior};

/// Primary *or* middle — the two buttons we treat interchangeably for
/// node body drag and selection.
const DRAG_BUTTONS: [PointerButton; 2] = [PointerButton::Primary, PointerButton::Middle];

/// Drag lifecycle events for a node body response, collapsed to a
/// single read of the response so the rest of `handle_node_interactions`
/// doesn't repeat the `primary || middle` check three times.
#[derive(Debug, Clone, Copy)]
struct NodeDragEvents {
    started: bool,
    dragging: bool,
    stopped: bool,
}

impl NodeDragEvents {
    fn from_response(r: &Response) -> Self {
        Self {
            started: DRAG_BUTTONS.iter().any(|&b| r.drag_started_by(b)),
            dragging: DRAG_BUTTONS.iter().any(|&b| r.dragged_by(b)),
            stopped: DRAG_BUTTONS.iter().any(|&b| r.drag_stopped_by(b)),
        }
    }
}

// ============================================================================
// Types
// ============================================================================

/// Variants are declared in ascending priority order — `prefer` folds
/// multiple port hits in a single frame down to the most-actionable
/// one (e.g. a `Click` wins over a `Hover`). `PortInfo` can't derive
/// `Ord` (it carries a `Pos2`), so priority is a discriminant-only
/// mapping rather than a derived `Ord`.
#[derive(Debug, Clone, Default)]
pub enum PortInteractCommand {
    #[default]
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
            Self::Hover(_) => 1,
            Self::DragStart(_) => 2,
            Self::DragStop => 3,
            Self::Click(_) => 4,
        }
    }

    fn prefer(&mut self, other: Self) {
        if other.priority() > self.priority() {
            *self = other;
        }
    }
}

fn exec_info_shadow<'a>(
    info: &NodeExecutionInfo<'_>,
    gui: &'a Gui<'_>,
) -> Option<&'a egui::Shadow> {
    match info {
        NodeExecutionInfo::Errored(_) => Some(&gui.style.node.errored_shadow),
        NodeExecutionInfo::MissingInputs => Some(&gui.style.node.missing_inputs_shadow),
        NodeExecutionInfo::Executed(_) => Some(&gui.style.node.executed_shadow),
        NodeExecutionInfo::Cached => Some(&gui.style.node.cached_shadow),
        NodeExecutionInfo::None => None,
    }
}

// ============================================================================
// Pass outputs
// ============================================================================

/// Produced by `render_nodes` and consumed by the orchestrator's
/// connection-handling pass. `removed_nodes` flows through actions
/// directly; only port + breaker-hit state is returned.
#[derive(Debug, Default)]
pub(crate) struct RenderNodesResult {
    pub port_cmd: PortInteractCommand,
    pub broken_nodes: Vec<NodeId>,
}

#[derive(Debug, Default)]
pub(crate) struct NodeUi {
    pub(crate) const_bind_ui: ConstBindUi,
}

impl NodeUi {
    /// Per-frame interaction pass for node bodies. Must run AFTER
    /// `GraphLayout::refresh_galleys` so every view-node has a galley
    /// entry and the current-frame `origin` is in place. The drag delta
    /// accumulated here feeds back into the gesture before
    /// `render_nodes` recomputes layouts for drawing.
    pub(crate) fn handle_node_interactions(
        &self,
        gui: &mut Gui<'_>,
        ctx: &GraphContext,
        graph_layout: &GraphLayout,
        output: &mut FrameOutput,
        gesture: &mut Gesture,
    ) {
        // A drag released on the previous frame kept its offset alive
        // through that frame's render (see `NodeDrag::released`). By
        // now `NodeMoved::apply` has run, so the offset is stale —
        // cancel before we read any drag state.
        if gesture.node_drag().is_some_and(|d| d.released) {
            gesture.cancel();
        }

        for view_node in ctx.view_graph.view_nodes.iter() {
            let node_id = view_node.id;
            let drag_offset = gesture.node_drag_offset_for(&node_id);
            let layout = graph_layout.node_layout(gui, ctx, &node_id, drag_offset);

            let body_id = StableId::new(("node_body", node_id)).id();
            let response = gui.ui().interact(
                layout.body_rect,
                body_id,
                Sense::click() | Sense::hover() | Sense::drag(),
            );
            let events = NodeDragEvents::from_response(&response);

            if (events.started || response.clicked())
                && ctx.view_graph.selected_node_id != Some(node_id)
            {
                output.add_action(GraphUiAction::NodeSelected {
                    before: ctx.view_graph.selected_node_id,
                    after: Some(node_id),
                });
            }

            if events.started {
                let start_pos = ctx.view_graph.view_nodes.by_key(&node_id).unwrap().pos;
                gesture.start_node_drag(node_id, start_pos);
            }
            if let Some(drag) = gesture.node_drag_mut()
                && drag.node_id == node_id
            {
                if events.dragging {
                    drag.offset += response.drag_delta() / gui.scale();
                }
                if events.stopped {
                    output.add_action(GraphUiAction::NodeMoved {
                        node_id,
                        before: drag.start_pos,
                        after: drag.committed_pos(),
                    });
                    // Keep offset alive for this frame's render; the
                    // cancel happens at the top of next frame's call.
                    drag.released = true;
                }
            }
        }
    }

    /// Per-frame render pass for node bodies + ports + labels. Emits
    /// `NodeRemoved` actions directly for clicked remove-buttons; port
    /// interaction commands and breaker hits flow back through the
    /// returned `RenderNodesResult`.
    pub(crate) fn render_nodes(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &GraphContext,
        graph_layout: &GraphLayout,
        output: &mut FrameOutput,
        gesture: &Gesture,
    ) -> RenderNodesResult {
        let mut result = RenderNodesResult::default();
        let mut const_bind_frame = self.const_bind_ui.start();
        let breaker = gesture.breaker();

        for view_node in ctx.view_graph.view_nodes.iter() {
            let node_id = view_node.id;
            let drag_offset = gesture.node_drag_offset_for(&node_id);
            let layout = graph_layout.node_layout(gui, ctx, &node_id, drag_offset);
            let galleys = graph_layout.node_galleys(&node_id);

            let node = ctx.view_graph.graph.by_id(&node_id).unwrap();
            let func = ctx.func_lib.by_id(&node.func_id).unwrap();

            const_bind_frame.render(gui, output, &layout, node, func, breaker);

            if !gui.ui().is_rect_visible(layout.body_rect) {
                continue;
            }

            let is_selected = ctx.view_graph.selected_node_id == Some(node_id);
            let exec_info = NodeExecutionInfo::from_stats(ctx.execution_stats, node_id);

            if render_body(gui, &layout, galleys, is_selected, &exec_info, breaker) {
                result.broken_nodes.push(node_id);
            }

            if render_remove_btn(gui, &layout, node_id) {
                output.add_action(ctx.view_graph.removal_action(&node_id));
            }

            render_status_hints(gui, &layout, node_id, node.behavior, func);
            render_cache_btn(gui, output, &layout, node);

            let missing_inputs = get_missing_input_ports(ctx.execution_stats, node_id);
            result
                .port_cmd
                .prefer(render_ports(gui, &layout, node, func, &missing_inputs));

            render_port_labels(gui, &layout, galleys);
        }

        const_bind_frame.finish();
        result
    }
}

// ============================================================================
// Body rendering
// ============================================================================

fn render_body(
    gui: &Gui<'_>,
    layout: &NodeLayout,
    galleys: &NodeGalleys,
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

    // Execution-state shadow
    if let Some(shadow) = exec_info_shadow(exec_info, gui) {
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
            (layout.header_row_height - galleys.title.size().y) * 0.5,
        );
    let title_color = breaker_hit.then_else(gui.style.dark_text_color, gui.style.text_color);
    gui.painter()
        .galley_with_override_text_color(title_pos, galleys.title.clone(), title_color);

    breaker_hit
}

// ============================================================================
// UI controls
// ============================================================================

fn render_cache_btn(gui: &mut Gui<'_>, output: &mut FrameOutput, layout: &NodeLayout, node: &Node) {
    if !layout.has_cache_btn {
        return;
    }

    let mut checked = node.behavior == NodeBehavior::Once;
    let response = Button::default()
        .toggle(&mut checked)
        .text("cache")
        .rect(layout.cache_button_rect)
        .show(gui, StableId::new(("cache_btn", node.id)));

    if response.clicked() {
        let before = node.behavior;
        let mut after = before;
        after.toggle();
        output.add_action(GraphUiAction::CacheToggled {
            node_id: node.id,
            before,
            after,
        });
    }
}

fn render_remove_btn(gui: &mut Gui<'_>, layout: &NodeLayout, node_id: NodeId) -> bool {
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
        .show(gui, StableId::new(("remove_btn", node_id)))
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
    let center = layout.status_dot_center;

    gui.painter()
        .circle_filled(center, dot_radius, gui.style.node.status_impure_color);

    // Tooltip on hover
    let dot_rect = Rect::from_center_size(center, vec2(dot_radius * 2.0, dot_radius * 2.0));
    let dot_id = gui.ui().make_persistent_id(("node_status_impure", node_id));
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

    let mut draw_port = |gui: &mut Gui<'_>, kind: PortKind, idx: usize, center: Pos2| {
        let port_rect = Rect::from_center_size(center, port_rect_size);
        let port_id = gui
            .ui()
            .make_persistent_id(("node_port", kind, node.id, idx));
        let response = gui.ui().interact(
            port_rect,
            port_id,
            Sense::click() | Sense::hover() | Sense::drag(),
        );
        let hovered = gui.ui().rect_contains_pointer(port_rect);

        if kind == PortKind::Input && missing_inputs.contains(&idx) {
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

        let info = PortInfo {
            port: PortRef {
                node_id: node.id,
                port_idx: idx,
                kind,
            },
            center,
        };
        let cmd = if response.drag_started_by(PointerButton::Primary) {
            PortInteractCommand::DragStart(info)
        } else if response.drag_stopped_by(PointerButton::Primary) {
            PortInteractCommand::DragStop
        } else if !response.dragged() && response.clicked_by(PointerButton::Primary) {
            PortInteractCommand::Click(info)
        } else if hovered {
            PortInteractCommand::Hover(info)
        } else {
            PortInteractCommand::None
        };
        result.prefer(cmd);
    };

    if func.terminal {
        draw_port(gui, PortKind::Trigger, 0, layout.trigger_center());
    }
    for idx in 0..func.inputs.len() {
        draw_port(gui, PortKind::Input, idx, layout.input_center(idx));
    }
    for idx in 0..func.outputs.len() {
        draw_port(gui, PortKind::Output, idx, layout.output_center(idx));
    }
    for idx in 0..func.events.len() {
        draw_port(gui, PortKind::Event, idx, layout.event_center(idx));
    }

    result
}

/// Which side of the port the label is drawn on. Inputs put the label to the
/// right of the port; outputs and events put it to the left.
#[derive(Copy, Clone)]
enum LabelSide {
    RightOfPort,
    LeftOfPort,
}

fn render_port_labels(gui: &Gui<'_>, layout: &NodeLayout, galleys: &NodeGalleys) {
    let padding = gui.style.node.port_label_side_padding;
    let color = gui.style.text_color;

    let draw = |galleys: &[std::sync::Arc<egui::Galley>],
                center: &dyn Fn(usize) -> egui::Pos2,
                side: LabelSide| {
        for (idx, galley) in galleys.iter().enumerate() {
            let size = galley.size();
            let x_offset = match side {
                LabelSide::RightOfPort => padding,
                LabelSide::LeftOfPort => -padding - size.x,
            };
            let pos = center(idx) + vec2(x_offset, -size.y * 0.5);
            gui.painter().galley(pos, galley.clone(), color);
        }
    };

    draw(
        &galleys.inputs,
        &|idx| layout.input_center(idx),
        LabelSide::RightOfPort,
    );
    draw(
        &galleys.outputs,
        &|idx| layout.output_center(idx),
        LabelSide::LeftOfPort,
    );
    draw(
        &galleys.events,
        &|idx| layout.event_center(idx),
        LabelSide::LeftOfPort,
    );
}
