//! Free-standing prepass scanners: plain `(hits, scene, ..)` functions
//! that turn one of [`CanvasHits`]' domain facts into the edit it means (a
//! graph to open, a file dialog to raise, a binding to toggle). None of
//! these touch [`crate::gui::node::NodeUI`]'s own state — that's the node
//! body's drag anchor, handled in `NodeUI::prepass` — so they live here
//! instead of crowding `node::mod` alongside the `NodeUI` struct.
//!
//! They read no responses of their own. Every widget poll behind them
//! happens once, in [`CanvasHits::scan`]; what is left here is the part
//! that needs the *scene* — resolving a chip's node to the graph it
//! opens, a clicked editor to the port config it picks for, a
//! double-clicked port to the bindings it severs.

use std::sync::Arc;

use scenarium::Binding;
use scenarium::InputPort;
use scenarium::{DataType, FsPathConfig, StaticValue};

use crate::core::document::{PortKind, PortRef};
use crate::core::edit::intent::sink::Intents;
use crate::gui::canvas::ctx::CanvasCtx;
use crate::gui::node::set_input;

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

/// Resolve a clicked const editor into a path pick, for the caller to
/// open a blocking file dialog after authoring.
///
/// Every const editor shares one widget family, so "which editor was
/// clicked" is all the scan can say; whether that click means *pick a
/// path* is a question about the port's type, and answering it needs the
/// scene. An editor on any other type has no button to click and falls
/// out here.
pub(crate) fn emit_path_picks(cx: CanvasCtx<'_>) -> Option<PathPickRequest> {
    let port = cx.hits().clicked_const_editor()?;
    let input = cx.graph_ctx().node(port.node_id)?.input(port.port_idx)?;
    if !matches!(
        input.binding(),
        Some(Binding::Const(
            StaticValue::FsPath(_) | StaticValue::FsPaths(_)
        ))
    ) {
        return None;
    }
    let DataType::FsPath(config) = input.ty() else {
        return None;
    };
    Some(PathPickRequest {
        port,
        config: config.clone(),
    })
}

/// Prepass scan: the port double-click. An input double-click (on the
/// port circle *or* its label) toggles the binding — clears it, or seeds
/// the default const when unbound; an output double-click disconnects
/// every consumer it feeds.
///
/// Emitted pre-record (like the connection commit) because adding or removing
/// a `Const` input's inline editor resizes the node — doing it before Pass A
/// lets the node arrange at its settled size and the wires re-anchor the same
/// frame, instead of floating until the relayout pass.
pub(crate) fn emit_port_dblclicks(cx: CanvasCtx<'_>, out: &mut Intents) {
    let Some(port) = cx.hits().double_clicked_port() else {
        return;
    };
    let Some(node) = cx.graph_ctx().node(port.node_id) else {
        return;
    };
    match port.kind {
        PortKind::Input => {
            let Some(input) = node.input(port.port_idx) else {
                return;
            };
            match input.binding() {
                // Unbound → seed the default literal (or first enum / value-
                // option variant, both resolved by `InputScope::default`).
                // Boundary ports route the interface — no const affordance, so
                // an unbound one has nothing to seed (its label double-click
                // renames).
                None => {
                    if let Some(default) = input.default() {
                        out.push(set_input(port, Binding::Const(default)));
                    }
                }
                // Already bound → clear it.
                Some(_) => out.push(set_input(port, None)),
            }
        }
        // An output may feed many inputs — clear each consumer.
        PortKind::Output => {
            for (consumer, producer) in cx.graph_ctx().connections() {
                if producer.node_id == port.node_id && producer.port_idx == port.port_idx {
                    out.push(set_input(
                        PortRef {
                            node_id: consumer.node_id,
                            kind: PortKind::Input,
                            port_idx: consumer.port_idx,
                        },
                        None,
                    ));
                }
            }
        }
    }
}
