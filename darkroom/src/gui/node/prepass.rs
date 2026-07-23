//! Free-standing prepass scanners: plain `(ui, scene, ..)` functions that
//! read last frame's chip/port responses into domain facts (a graph to
//! open, a node to run, a file dialog to open, a binding to toggle). None
//! of these touch [`crate::gui::node::NodeUI`]'s own state — that's the
//! node body's drag anchor, handled in `NodeUI::prepass` — so they live
//! here instead of crowding `node::mod` alongside the `NodeUI` struct.

use std::sync::Arc;

use aperture::Ui;
use scenarium::Binding;
use scenarium::GraphLink;
use scenarium::InputPort;
use scenarium::NodeId;
use scenarium::{DataType, FsPathConfig, StaticValue};

use crate::core::document::GraphRef;
use crate::core::document::{PortKind, PortRef};
use crate::core::edit::intent::types::Intent;
use crate::gui::UiAction;
use crate::gui::canvas::node_ports;
use crate::gui::node::header::{graph_badge_wid, play_badge_wid};
use crate::gui::node::port_row::{const_editor_wid, input_cell_wid, port_circle_wid};
use crate::gui::node::set_input;
use crate::gui::scene::{InputBindingView, Scene};

/// Prepass scan: surface an `OpenGraph` for any graph node whose `G`
/// chip was clicked (read from last frame's response). Detecting the
/// open here — *before* the record — lets `App` switch the active graph
/// ahead of Pass A, so the graph records a pass earlier and its
/// connections draw with no first-frame gap. Linked graphs aren't
/// editable targets yet, so only `Local` opens.
pub(crate) fn emit_graph_opens(ui: &Ui, scene: &Scene, actions: &mut Vec<UiAction>) {
    for n in scene.nodes.values() {
        // Instances are always `Local` (library graphs are localized on
        // instance), so the "G" chip opens the graph directly.
        if let Some(GraphLink::Local(id)) = n.graph
            && ui.response_for(graph_badge_wid(n.id)).left.clicked()
        {
            actions.push(UiAction::OpenGraph(GraphRef::Local(id)));
        }
    }
}

/// Scan for a click on a node's header play chip (read from last frame's
/// response), returning the node to run to. First hit wins — one run per
/// frame. The node UI surfaces only the domain fact (which node); the
/// canvas translates it into the run command. The `runnable` guard matches
/// where the chip draws, so a stale response can't seed an unrunnable node.
pub(crate) fn emit_play_clicks(ui: &Ui, scene: &Scene) -> Option<NodeId> {
    scene
        .nodes
        .values()
        .find(|n| n.runnable() && ui.response_for(play_badge_wid(n.id)).left.clicked())
        .map(|n| n.id)
}

/// A click on an `FsPath` input's inline pick button, surfaced for the
/// caller to translate into a file-dialog command. The node UI
/// produces the domain request (node + port + picker config) and stays
/// unaware of the app-level `AppCommand` enum — the canvas, which already
/// owns the command channel, does the translation.
#[derive(Clone, Debug)]
pub(crate) struct PathPickRequest {
    pub(crate) port: InputPort,
    /// The picker config is type-level metadata, taken from the port's
    /// `DataType` (the value only carries the selected path strings).
    pub(crate) config: Arc<FsPathConfig>,
}

/// Scan for a click on an `FsPath` input's inline pick button (polled by
/// its const-editor id, from last frame's responses). Returns the first
/// hit — one pick per frame — for the caller to open a blocking file dialog
/// after authoring.
pub(crate) fn emit_path_picks(ui: &Ui, scene: &Scene) -> Option<PathPickRequest> {
    for node in scene.nodes.values() {
        for (port_idx, input) in scene.inputs(node.inputs).iter().enumerate() {
            let port = InputPort::new(node.id, port_idx);
            if matches!(
                &input.binding,
                InputBindingView::Const(StaticValue::FsPath(_) | StaticValue::FsPaths(_))
            ) && let DataType::FsPath(config) = &input.ty
                && ui.response_for(const_editor_wid(port)).left.clicked()
            {
                return Some(PathPickRequest {
                    port,
                    config: config.clone(),
                });
            }
        }
    }
    None
}

/// Prepass scan: port double-clicks read from last frame's responses. An
/// input double-click (on the port circle *or* its label) toggles the
/// binding — clears it, or seeds the default const when unbound; an output
/// double-click disconnects every consumer it feeds.
///
/// Emitted pre-record (like the connection commit) because adding or removing
/// a `Const` input's inline editor resizes the node — doing it before Pass A
/// lets the node arrange at its settled size and the wires re-anchor the same
/// frame, instead of floating until the relayout pass.
pub(crate) fn emit_port_dblclicks(ui: &Ui, scene: &Scene, out: &mut Vec<Intent>) {
    for node in scene.nodes.values() {
        // Boundary ports route the interface — no const affordance, so an
        // unbound one has nothing to seed (its label double-click renames).
        let can_set = !node.boundary;
        for (i, input) in scene.inputs(node.inputs).iter().enumerate() {
            let port = PortRef {
                node_id: node.id,
                kind: PortKind::Input,
                port_idx: i,
            };
            // The circle intercepts its own rect; the cell catches the label.
            let dbl = ui.response_for(port_circle_wid(port)).left.double_clicked()
                || ui.response_for(input_cell_wid(port)).left.double_clicked();
            if !dbl {
                continue;
            }
            match &input.binding {
                // Unbound → seed the default literal (or first enum / value-
                // option variant, both already folded into `SceneInput::default`).
                InputBindingView::None => {
                    if can_set && let Some(default) = &input.default {
                        out.push(set_input(port, Binding::Const(default.clone())));
                    }
                }
                // Already bound → clear it.
                _ => out.push(set_input(port, None)),
            }
        }
        for port in node_ports(node, PortKind::Output) {
            if ui.response_for(port_circle_wid(port)).left.double_clicked() {
                // An output may feed many inputs — clear each consumer.
                for c in &scene.connections {
                    if c.src.node_id == port.node_id && c.src.port_idx == port.port_idx {
                        out.push(set_input(
                            PortRef {
                                node_id: c.tgt.node_id,
                                kind: PortKind::Input,
                                port_idx: c.tgt.port_idx,
                            },
                            None,
                        ));
                    }
                }
            }
        }
    }
}
