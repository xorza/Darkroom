use std::collections::BTreeSet;

use glam::Vec2;
use palantir::InternedStr;
use scenarium::data::StaticValue;
use scenarium::prelude::{Binding, FuncId, FuncLib, NodeId};

use crate::document::Document;

#[derive(Debug)]
pub struct Scene {
    pub nodes: Vec<SceneNode>,
    pub connections: Vec<SceneConnection>,
    /// Flat pool of port-name handles. Each `SceneNode` slices into it
    /// via `inputs` / `outputs` spans — keeps per-node allocations to
    /// zero in steady state.
    pub port_names: Vec<InternedStr>,
    /// Flat pool of input-binding snapshots, one per input port across
    /// every node. `SceneNode::input_bindings` slices into it (same len
    /// as `SceneNode::inputs`).
    pub input_bindings: Vec<InputBindingView>,
    /// Live viewport, mirrored from `Document::{pan, scale}` each
    /// `rebuild`. Read by the canvas transform and the pointer↔world
    /// mapping; the pan/zoom gesture writes it back here, and `App`
    /// copies it onto the document so it persists (single owner =
    /// `Document`).
    pub pan: Vec2,
    pub zoom: f32,
    /// Currently-selected nodes, mirrored from `Document` each rebuild
    /// so `node_ui` can pick a different paint without taking a `&Document`.
    pub selected_nodes: BTreeSet<NodeId>,
}

/// Per-frame snapshot of an input port's [`Binding`] for the UI tree.
/// Variant-only for `Bind`; the address details live on `Scene::connections`.
#[derive(Debug, Clone)]
pub enum InputBindingView {
    None,
    Const(StaticValue),
    Bind,
}

impl From<&Binding> for InputBindingView {
    fn from(b: &Binding) -> Self {
        match b {
            Binding::None => Self::None,
            Binding::Const(v) => Self::Const(v.clone()),
            Binding::Bind(_) => Self::Bind,
        }
    }
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            connections: Vec::new(),
            port_names: Vec::new(),
            input_bindings: Vec::new(),
            pan: Vec2::ZERO,
            zoom: 1.0,
            selected_nodes: BTreeSet::new(),
        }
    }
}

#[derive(Debug)]
pub struct SceneNode {
    pub id: NodeId,
    pub func_id: FuncId,
    pub pos: Vec2,
    pub name: InternedStr,
    pub inputs: PortSpan,
    pub outputs: PortSpan,
    pub input_bindings: PortSpan,
}

#[derive(Debug)]
pub struct SceneConnection {
    pub src_node: NodeId,
    pub src_port: usize,
    pub tgt_node: NodeId,
    pub tgt_port: usize,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct PortSpan {
    pub start: u32,
    pub len: u32,
}

impl Scene {
    /// Names live in palantir's per-frame text arena, which clears at
    /// the next `Ui::frame` — so `Scene` must be rebuilt every frame
    /// before any widget consumes it. `App::frame` enforces this.
    pub fn rebuild(&mut self, doc: &Document, func_lib: &FuncLib) {
        self.selected_nodes = doc.selected_nodes.clone();
        // Mirror the persisted viewport. The gesture overwrites these
        // later this frame and `App` copies them back onto the doc, so
        // a fresh value (e.g. just-loaded document) shows up here while
        // an active pan/zoom isn't clobbered (it re-persists each frame).
        self.pan = doc.pan;
        self.zoom = doc.scale;
        self.nodes.clear();
        self.connections.clear();
        self.port_names.clear();
        self.input_bindings.clear();

        for vn in doc.view_nodes.iter() {
            let Some(node) = doc.graph.by_id(&vn.id) else {
                continue;
            };
            // Stage 1: only func nodes are rendered; subgraph instances get
            // their own UI in a later stage.
            let Some(func_id) = node.func_id() else {
                continue;
            };
            let Some(func) = func_lib.by_id(&func_id) else {
                continue;
            };
            let inputs = extend_pool(
                &mut self.port_names,
                func.inputs.iter().map(|i| i.name.clone().into()),
            );
            let outputs = extend_pool(
                &mut self.port_names,
                func.outputs.iter().map(|o| o.name.clone().into()),
            );
            let input_bindings = extend_pool(
                &mut self.input_bindings,
                node.inputs
                    .iter()
                    .map(|i| InputBindingView::from(&i.binding)),
            );
            self.nodes.push(SceneNode {
                id: vn.id,
                func_id,
                pos: vn.pos,
                name: node.name.clone().into(),
                inputs,
                outputs,
                input_bindings,
            });
        }

        for node in doc.graph.iter() {
            for (tgt_port, inp) in node.inputs.iter().enumerate() {
                let Binding::Bind(addr) = &inp.binding else {
                    continue;
                };
                self.connections.push(SceneConnection {
                    src_node: addr.node_id,
                    src_port: addr.port_idx,
                    tgt_node: node.id,
                    tgt_port,
                });
            }
        }
    }

    pub fn ports(&self, span: PortSpan) -> &[InternedStr] {
        let start = span.start as usize;
        &self.port_names[start..start + span.len as usize]
    }

    pub fn bindings(&self, span: PortSpan) -> &[InputBindingView] {
        let start = span.start as usize;
        &self.input_bindings[start..start + span.len as usize]
    }
}

fn extend_pool<T>(pool: &mut Vec<T>, items: impl IntoIterator<Item = T>) -> PortSpan {
    let start = pool.len();
    pool.extend(items);
    PortSpan {
        start: start as u32,
        len: (pool.len() - start) as u32,
    }
}
