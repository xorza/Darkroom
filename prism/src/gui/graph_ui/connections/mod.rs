//! Renderer for permanent and in-flight graph connections. The drag
//! state machine and action emission live in `actions.rs`; pure data
//! types live in `types.rs`.

pub mod actions;
pub mod bezier;
pub mod breaker;
pub mod handlers;
pub mod types;

use common::key_index_vec::{CompactInsert, KeyIndexKey, KeyIndexVec};
use egui::{PointerButton, Pos2, Sense};
use scenarium::graph::PortAddress;
use scenarium::prelude::Binding;
use scenarium::worker::EventRef;

use crate::gui::Gui;
use crate::gui::graph_ui::connections::actions::{apply_connection_deletions, order_ports};
use crate::gui::graph_ui::connections::bezier::{ConnectionBezier, ConnectionBezierStyle};
use crate::gui::graph_ui::connections::breaker::ConnectionBreaker;
use crate::gui::graph_ui::ctx::GraphContext;
use crate::gui::graph_ui::frame_output::FrameOutput;
use crate::gui::graph_ui::gesture::Gesture;
use crate::gui::graph_ui::layout::GraphLayout;
use crate::gui::graph_ui::port::{PortKind, PortRef};

pub(crate) use types::{
    BrokeItem, ConnectionCurve, ConnectionDrag, ConnectionDragUpdate, ConnectionKey,
};

/// Decoration-only bezier keyed by the connection it overlays.
/// Used for "missing input" / "triggered event" highlights — no
/// interaction, so no `broke` / `hovered` state.
#[derive(Debug)]
struct HighlightCurve {
    key: ConnectionKey,
    bezier: ConnectionBezier,
}

impl KeyIndexKey<ConnectionKey> for HighlightCurve {
    fn key(&self) -> &ConnectionKey {
        &self.key
    }
}

#[derive(Debug, Default)]
pub(crate) struct ConnectionUi {
    curves: KeyIndexVec<ConnectionKey, ConnectionCurve>,
    highlight_curves: KeyIndexVec<ConnectionKey, HighlightCurve>,

    /// Cached mesh buffer for the in-flight drag preview — reused across
    /// frames while a drag is active. The drag data itself lives in
    /// [`crate::gui::graph_ui::gesture::Gesture::DraggingConnection`].
    temp_connection_bezier: ConnectionBezier,
}

impl ConnectionUi {
    pub(crate) fn render(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &GraphContext,
        graph_layout: &GraphLayout,
        gesture: &Gesture,
        output: &mut FrameOutput,
        breaker: Option<&ConnectionBreaker>,
    ) {
        let execution_stats = ctx.execution_stats;
        let mut curves = self.curves.compact_insert_start();
        let mut highlights = self.highlight_curves.compact_insert_start();
        let mut deletions: Vec<ConnectionKey> = Vec::new();

        for node_view in &ctx.view_graph.view_nodes {
            let node_id = node_view.id;
            let node = ctx.view_graph.graph.by_id(&node_id).unwrap();
            let vp = gui.view_params();
            let node_layout = graph_layout.node_layout(
                &vp,
                ctx,
                &node_id,
                gesture.node_drag_offset_for(&node_id),
            );

            // Render data connections
            for (input_idx, input) in node.inputs.iter().enumerate() {
                let Binding::Bind(binding) = &input.binding else {
                    continue;
                };

                let key = ConnectionKey::Input {
                    input_node_id: node_id,
                    input_idx,
                };
                let output_layout = graph_layout.node_layout(
                    &vp,
                    ctx,
                    &binding.target_id,
                    gesture.node_drag_offset_for(&binding.target_id),
                );
                let input_pos = node_layout.input_center(input_idx);
                let output_pos = output_layout.output_center(binding.port_idx);

                let is_missing = execution_stats.is_some_and(|stats| {
                    stats.missing_inputs.contains(&PortAddress {
                        target_id: node_id,
                        port_idx: input_idx,
                    })
                });

                if is_missing {
                    render_highlight_curve(gui, &mut highlights, key, output_pos, input_pos);
                }

                update_curve_interaction(
                    gui,
                    &mut curves,
                    &mut deletions,
                    key,
                    output_pos,
                    input_pos,
                    PortKind::Input,
                    breaker,
                );
            }

            // Render event connections
            for (event_idx, event) in node.events.iter().enumerate() {
                let event_pos = node_layout.event_center(event_idx);

                for &trigger_node_id in &event.subscribers {
                    let trigger_layout = graph_layout.node_layout(
                        &vp,
                        ctx,
                        &trigger_node_id,
                        gesture.node_drag_offset_for(&trigger_node_id),
                    );
                    let trigger_pos = trigger_layout.trigger_center();

                    let key = ConnectionKey::Event {
                        event_node_id: node_id,
                        event_idx,
                        trigger_node_id,
                    };

                    let is_triggered = execution_stats.is_some_and(|stats| {
                        stats
                            .triggered_events
                            .contains(&EventRef { node_id, event_idx })
                    });

                    if is_triggered {
                        render_highlight_curve(gui, &mut highlights, key, event_pos, trigger_pos);
                    }

                    update_curve_interaction(
                        gui,
                        &mut curves,
                        &mut deletions,
                        key,
                        event_pos,
                        trigger_pos,
                        PortKind::Trigger,
                        breaker,
                    );
                }
            }
        }

        drop(curves);
        drop(highlights);

        apply_connection_deletions(deletions, ctx, output);
    }

    /// Draws the in-flight connection preview for a drag owned by
    /// [`crate::gui::graph_ui::gesture::Gesture`].
    pub(crate) fn render_temp_connection(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &GraphContext<'_>,
        graph_layout: &GraphLayout,
        drag: &ConnectionDrag,
    ) {
        // Port centers come from fresh layout — a connection drag doesn't
        // coexist with a node drag, so drag_offset is always zero here.
        let vp = gui.view_params();
        let port_center = |port: &PortRef| {
            let layout = graph_layout.node_layout(&vp, ctx, &port.node_id, egui::Vec2::ZERO);
            layout.port_center(port)
        };
        let start_center = port_center(&drag.start_port);

        // Determine bezier direction based on port kind
        let (start, end) = if drag.start_port.kind.is_source() {
            (start_center, drag.current_pos)
        } else {
            (drag.current_pos, start_center)
        };

        self.temp_connection_bezier
            .update_points(start, end, gui.scale());
        // ID-salt trick: when the drag is snapped to a compatible
        // end port, use the same salt that the permanent connection
        // will use once committed. Without this, the frame on which
        // the permanent connection appears would register a widget at
        // the same rect with a different id than last frame's
        // temp-connection, tripping egui's "Widget rect changed id
        // between passes" warning (red-rect flash).
        let snapped_key = drag.end_port.map(|end| {
            let (input_port, output_port) = order_ports(drag.start_port, end);
            match output_port.kind {
                PortKind::Output => ConnectionKey::Input {
                    input_node_id: input_port.node_id,
                    input_idx: input_port.port_idx,
                },
                PortKind::Event => ConnectionKey::Event {
                    event_node_id: output_port.node_id,
                    event_idx: output_port.port_idx,
                    trigger_node_id: input_port.node_id,
                },
                _ => unreachable!("end port kinds are validated by try_snap_to_port"),
            }
        });
        let style = ConnectionBezierStyle::build(&gui.style, drag.start_port.kind, false, false);
        if let Some(key) = snapped_key {
            self.temp_connection_bezier
                .show(gui, Sense::hover(), ("connection", key), style);
        } else {
            self.temp_connection_bezier
                .show(gui, Sense::hover(), "temp_connection", style);
        }
    }

    pub(crate) fn any_hovered(&self) -> bool {
        self.curves.iter().any(|c| c.hovered)
    }

    pub(crate) fn broke_iter(&self) -> impl Iterator<Item = &ConnectionKey> {
        self.curves
            .iter()
            .filter_map(|curve| curve.broke.then_some(&curve.key))
    }
}

fn get_or_create_curve<'a>(
    compact: &'a mut CompactInsert<'_, ConnectionKey, ConnectionCurve>,
    key: ConnectionKey,
) -> &'a mut ConnectionCurve {
    compact.insert_with(&key, || ConnectionCurve::new(key)).1
}

/// Shared update-and-interact pass for one connection curve.
///
/// Used by both data (input-binding) and event (trigger) connections — they
/// differ only in which endpoints feed `start_pos` / `end_pos` and which
/// `port_kind` is used for styling.
#[allow(clippy::too_many_arguments)]
fn update_curve_interaction(
    gui: &mut Gui<'_>,
    curves: &mut CompactInsert<'_, ConnectionKey, ConnectionCurve>,
    deletions: &mut Vec<ConnectionKey>,
    key: ConnectionKey,
    start_pos: Pos2,
    end_pos: Pos2,
    port_kind: PortKind,
    breaker: Option<&ConnectionBreaker>,
) {
    let curve = get_or_create_curve(curves, key);
    curve.bezier.update_points(start_pos, end_pos, gui.scale());
    curve.broke = curve.bezier.intersects_breaker(breaker);

    let response = show_curve(gui, curve, port_kind);

    if breaker.is_some() {
        curve.hovered = false;
    } else {
        curve.hovered = response.hovered();
        if response.double_clicked_by(PointerButton::Primary) {
            deletions.push(key);
            curve.hovered = false;
        }
    }
}

fn render_highlight_curve(
    gui: &mut Gui<'_>,
    highlights: &mut CompactInsert<'_, ConnectionKey, HighlightCurve>,
    key: ConnectionKey,
    start_pos: Pos2,
    end_pos: Pos2,
) {
    let (_, curve) = highlights.insert_with(&key, || HighlightCurve {
        key,
        bezier: ConnectionBezier::default(),
    });
    curve.bezier.update_points(start_pos, end_pos, gui.scale());

    let style = ConnectionBezierStyle {
        start_color: gui.style.node.missing_inputs_shadow.color,
        end_color: gui.style.node.missing_inputs_shadow.color,
        stroke_width: gui.style.connections.stroke_width,
        feather: gui.style.connections.highlight_feather,
    };

    curve
        .bezier
        .show(gui, Sense::empty(), ("connection_highlight", key), style);
}

fn show_curve(
    gui: &mut Gui<'_>,
    curve: &mut ConnectionCurve,
    port_kind: PortKind,
) -> egui::Response {
    let style = ConnectionBezierStyle::build(&gui.style, port_kind, curve.broke, curve.hovered);
    curve.bezier.show(
        gui,
        Sense::click() | Sense::hover(),
        ("connection", curve.key),
        style,
    )
}
