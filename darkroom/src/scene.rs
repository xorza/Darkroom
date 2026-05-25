use std::collections::BTreeSet;

use glam::Vec2;
use palantir::InternedStr;
use scenarium::data::StaticValue;
use scenarium::function::FuncInput;
use scenarium::prelude::{Binding, FuncLib, NodeBehavior, NodeId, NodeKind};

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
    /// Flat pool of each input port's default literal (from the func/def
    /// interface), resolved once per rebuild. Sliced by the same span as
    /// `input_bindings`, so the UI can offer a "set constant" value
    /// without re-resolving against the func lib (and without needing a
    /// func at all for subgraph instance nodes).
    pub input_defaults: Vec<StaticValue>,
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
            input_defaults: Vec::new(),
            pan: Vec2::ZERO,
            zoom: 1.0,
            selected_nodes: BTreeSet::new(),
        }
    }
}

#[derive(Debug)]
pub struct SceneNode {
    pub id: NodeId,
    pub pos: Vec2,
    pub name: InternedStr,
    pub inputs: PortSpan,
    pub outputs: PortSpan,
    pub input_bindings: PortSpan,
    /// Composite (`NodeKind::Subgraph`) instance vs a plain func node.
    pub is_subgraph: bool,
    /// Sink node (its func is `terminal` — no outputs feed downstream).
    pub terminal: bool,
    /// Result is cached / computed once (`NodeBehavior::Once`). The
    /// header badge toggles this via `Intent::SetCacheBehavior`.
    pub cached: bool,
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
        self.input_defaults.clear();

        for vn in doc.view_nodes.iter() {
            let Some(node) = doc.graph.by_id(&vn.id) else {
                continue;
            };
            // A node's interface (port names + input defaults) comes from
            // its func or its subgraph def — both expose `FuncInput`s /
            // `FuncOutput`s. Boundary nodes only exist inside a def's
            // interior, never at the top level rendered here.
            let interface = match &node.kind {
                NodeKind::Func(func_id) => func_lib.by_id(func_id).map(|f| NodeInterface {
                    inputs: &f.inputs,
                    output_names: f.outputs.iter().map(|o| o.name.clone()).collect(),
                    is_subgraph: false,
                    terminal: f.terminal,
                }),
                NodeKind::Subgraph(r) => {
                    doc.graph.resolve_def(*r, func_lib).map(|d| NodeInterface {
                        inputs: &d.inputs,
                        output_names: d.outputs.iter().map(|o| o.name.clone()).collect(),
                        is_subgraph: true,
                        // A composite's terminal-ness is derived at flatten
                        // time, not stored on the def; treat "no exposed
                        // outputs" as the visible sink signal.
                        terminal: d.outputs.is_empty(),
                    })
                }
                NodeKind::SubgraphInput | NodeKind::SubgraphOutput => None,
            };
            let Some(interface) = interface else {
                continue;
            };
            let inputs = extend_pool(
                &mut self.port_names,
                interface.inputs.iter().map(|i| i.name.clone().into()),
            );
            let outputs = extend_pool(
                &mut self.port_names,
                interface.output_names.iter().map(|n| n.clone().into()),
            );
            let input_bindings = extend_pool(
                &mut self.input_bindings,
                doc.graph
                    .node_bindings(node.id, interface.inputs.len())
                    .map(|(_, binding)| InputBindingView::from(&binding)),
            );
            extend_pool(
                &mut self.input_defaults,
                interface.inputs.iter().map(default_static_value),
            );
            self.nodes.push(SceneNode {
                id: vn.id,
                pos: vn.pos,
                name: node.name.clone().into(),
                inputs,
                outputs,
                input_bindings,
                is_subgraph: interface.is_subgraph,
                terminal: interface.terminal,
                cached: node.behavior == NodeBehavior::Once,
            });
        }

        for (dst, src) in doc.graph.edges() {
            self.connections.push(SceneConnection {
                src_node: src.node_id,
                src_port: src.port_idx,
                tgt_node: dst.node_id,
                tgt_port: dst.port_idx,
            });
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

    /// Per-input default literals, sliced by the node's `input_bindings`
    /// span (defaults are pushed in lockstep with bindings, so the spans
    /// coincide).
    pub fn defaults(&self, span: PortSpan) -> &[StaticValue] {
        let start = span.start as usize;
        &self.input_defaults[start..start + span.len as usize]
    }
}

/// Borrowed view of a node's interface during a single rebuild: the
/// input ports (whole `FuncInput`, for names + defaults) and the output
/// port names. Sourced from a func or a subgraph def uniformly.
struct NodeInterface<'a> {
    inputs: &'a [FuncInput],
    output_names: Vec<String>,
    is_subgraph: bool,
    terminal: bool,
}

/// The literal a port falls back to when given a const binding: its
/// declared default, else the zero value for its data type.
fn default_static_value(input: &FuncInput) -> StaticValue {
    input
        .default_value
        .clone()
        .unwrap_or_else(|| StaticValue::from(&input.data_type))
}

fn extend_pool<T>(pool: &mut Vec<T>, items: impl IntoIterator<Item = T>) -> PortSpan {
    let start = pool.len();
    pool.extend(items);
    PortSpan {
        start: start as u32,
        len: (pool.len() - start) as u32,
    }
}
