use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};

use glam::Vec2;
use palantir::InternedStr;
use scenarium::data::{DataType, StaticValue};
use scenarium::function::{FuncInput, FuncOutput};
use scenarium::prelude::{
    Binding, FuncLib, Graph, NodeBehavior, NodeId, NodeKind, SubgraphDef, SubgraphRef,
};

use crate::document::GraphView;
use crate::exec_status::ExecStatus;

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
    /// `Some` for a composite (`NodeKind::Subgraph`) instance — carries
    /// the ref so the header's open-in-tab action knows which def to
    /// target. `None` for a plain func node.
    pub subgraph: Option<SubgraphRef>,
    /// Sink node (its func is `terminal` — no outputs feed downstream).
    pub terminal: bool,
    /// Result is cached / computed once (`NodeBehavior::Once`). The
    /// header badge toggles this via `Intent::SetCacheBehavior`.
    pub cached: bool,
    /// A `SubgraphInput`/`SubgraphOutput` interface boundary node. Its
    /// ports route the subgraph interface rather than carry literal
    /// values, so the const-value affordances (inline editor, "Set
    /// constant" menu, drag-to-own-body) are suppressed on them.
    pub boundary: bool,
    /// Outcome of the last graph run, mirrored from the worker's
    /// `ExecutionStats`. Drives the node's status-glow shadow and (for
    /// `Executed`) the header time label; `None` (the default) paints
    /// no glow.
    pub exec_status: ExecStatus,
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
    ///
    /// `ctx_def` is the enclosing `SubgraphDef` when `graph` is a
    /// subgraph interior, `None` for the root. It's the only source of
    /// port arity for the `SubgraphInput`/`SubgraphOutput` boundary nodes
    /// (they carry no func) — their ports mirror the def's interface.
    pub fn rebuild(
        &mut self,
        graph: &Graph,
        view: &GraphView,
        func_lib: &FuncLib,
        ctx_def: Option<&SubgraphDef>,
        exec_status: &HashMap<NodeId, ExecStatus>,
    ) {
        self.selected_nodes = view.selected_nodes.clone();
        // Mirror the persisted viewport. The gesture overwrites these
        // later this frame and `App` copies them back onto the doc, so
        // a fresh value (e.g. just-loaded document) shows up here while
        // an active pan/zoom isn't clobbered (it re-persists each frame).
        self.pan = view.pan;
        self.zoom = view.scale;
        self.nodes.clear();
        self.connections.clear();
        self.port_names.clear();
        self.input_bindings.clear();
        self.input_defaults.clear();

        for vn in view.view_nodes.iter() {
            let Some(node) = graph.by_id(&vn.id) else {
                continue;
            };
            // A node's interface (port names + input defaults) comes from
            // its func or its subgraph def — both expose `FuncInput`s /
            // `FuncOutput`s. The two boundary kinds only exist inside a
            // def's interior; they carry no func, so their port arity is
            // mirrored from the enclosing `ctx_def`'s interface.
            let interface = match &node.kind {
                NodeKind::Func(func_id) => func_lib.by_id(func_id).map(|f| NodeInterface {
                    inputs: Cow::Borrowed(&f.inputs),
                    output_names: f.outputs.iter().map(|o| o.name.clone()).collect(),
                    subgraph: None,
                    terminal: f.terminal,
                }),
                NodeKind::Subgraph(r) => graph.resolve_def(*r, func_lib).map(|d| NodeInterface {
                    inputs: Cow::Borrowed(&d.inputs),
                    output_names: d.outputs.iter().map(|o| o.name.clone()).collect(),
                    subgraph: Some(*r),
                    // A composite's terminal-ness is derived at flatten
                    // time, not stored on the def; treat "no exposed
                    // outputs" as the visible sink signal.
                    terminal: d.outputs.is_empty(),
                }),
                // Inbound boundary: no inputs; one output per def input,
                // plus a trailing placeholder output. Dragging from the
                // placeholder commits a normal `SetInput` binding to its
                // index; `reconcile_boundaries` then grows `def.inputs` to
                // match and a fresh placeholder appears next frame.
                NodeKind::SubgraphInput => ctx_def.map(|d| {
                    let mut output_names: Vec<String> =
                        d.inputs.iter().map(|i| i.name.clone()).collect();
                    output_names.push(PLACEHOLDER_PORT.to_string());
                    NodeInterface {
                        inputs: Cow::Borrowed(&[]),
                        output_names,
                        subgraph: None,
                        terminal: false,
                    }
                }),
                // Outbound boundary: one input per def output (synthesized
                // as `FuncInput`s for names + zero defaults), plus a
                // trailing placeholder input. Symmetric to the inbound
                // case — wiring the placeholder grows `def.outputs`.
                NodeKind::SubgraphOutput => ctx_def.map(|d| {
                    let mut inputs: Vec<FuncInput> = d.outputs.iter().map(boundary_input).collect();
                    inputs.push(placeholder_input());
                    NodeInterface {
                        inputs: Cow::Owned(inputs),
                        output_names: Vec::new(),
                        subgraph: None,
                        terminal: false,
                    }
                }),
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
                graph
                    .node_bindings(node.id, interface.inputs.len())
                    .map(|(_, binding)| InputBindingView::from(&binding)),
            );
            extend_pool(
                &mut self.input_defaults,
                interface.inputs.iter().map(default_static_value),
            );
            // Boundary nodes carry no name in the model; label them by
            // role so the interior header isn't a blank bar.
            let name: InternedStr = match (&node.kind, node.name.is_empty()) {
                (NodeKind::SubgraphInput, true) => "Inputs".into(),
                (NodeKind::SubgraphOutput, true) => "Outputs".into(),
                _ => node.name.clone().into(),
            };
            self.nodes.push(SceneNode {
                id: vn.id,
                pos: vn.pos,
                name,
                inputs,
                outputs,
                input_bindings,
                subgraph: interface.subgraph,
                terminal: interface.terminal,
                cached: node.behavior == NodeBehavior::Once,
                boundary: matches!(
                    node.kind,
                    NodeKind::SubgraphInput | NodeKind::SubgraphOutput
                ),
                exec_status: exec_status.get(&vn.id).copied().unwrap_or_default(),
            });
        }

        for (dst, src) in graph.edges() {
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

/// View of a node's interface during a single rebuild: the input ports
/// (whole `FuncInput`, for names + defaults) and the output port names.
/// Inputs are usually borrowed from a func/def; the `SubgraphOutput`
/// boundary node synthesizes them from the def's `FuncOutput`s, hence
/// `Cow`.
struct NodeInterface<'a> {
    inputs: Cow<'a, [FuncInput]>,
    output_names: Vec<String>,
    subgraph: Option<SubgraphRef>,
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

/// Synthesize a `FuncInput` for a `SubgraphOutput`'s input port from the
/// def output it mirrors — name + type carry over; it's not user-set, so
/// it has no declared default and no value options.
fn boundary_input(output: &FuncOutput) -> FuncInput {
    FuncInput {
        name: output.name.clone(),
        required: false,
        data_type: output.data_type.clone(),
        default_value: None,
        value_options: Vec::new(),
    }
}

/// Label for the trailing "connect here to add a port" placeholder on a
/// boundary node.
const PLACEHOLDER_PORT: &str = "+";

/// The placeholder input port for a `SubgraphOutput`: unbound, untyped
/// until something connects (at which point reconcile materializes a real
/// `def.outputs` entry from the wired producer's type).
fn placeholder_input() -> FuncInput {
    FuncInput {
        name: PLACEHOLDER_PORT.to_string(),
        required: false,
        data_type: DataType::default(),
        default_value: None,
        value_options: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::data::DataType;
    use scenarium::prelude::{InputPort, Node, SubgraphDef};

    fn finput(name: &str, ty: DataType) -> FuncInput {
        FuncInput {
            name: name.into(),
            required: false,
            data_type: ty,
            default_value: None,
            value_options: Vec::new(),
        }
    }

    /// `def`: inputs A:Int, B:Float → outputs Sum:Int. Interior wires the
    /// inbound boundary straight to the outbound one
    /// (`SubgraphInput.out[0]` → `SubgraphOutput.in[0]`). Returns the def
    /// plus the two boundary node ids so tests can locate them in `Scene`.
    fn adder_def() -> (SubgraphDef, NodeId, NodeId) {
        let in_node = Node::new(NodeKind::SubgraphInput);
        let in_id = in_node.id;
        let out_node = Node::new(NodeKind::SubgraphOutput);
        let out_id = out_node.id;
        let mut inner = Graph::default();
        inner.add(in_node);
        inner.add(out_node);
        inner.set_input_binding(InputPort::new(out_id, 0), (in_id, 0).into());

        let def = SubgraphDef {
            id: "00000000-0000-0000-0000-000000000000".into(),
            name: "Adder".into(),
            category: "Subgraph".into(),
            graph: inner,
            inputs: vec![finput("A", DataType::Int), finput("B", DataType::Float)],
            outputs: vec![FuncOutput {
                name: "Sum".into(),
                data_type: DataType::Int,
            }],
            events: vec![],
        };
        (def, in_id, out_id)
    }

    #[test]
    fn boundary_nodes_mirror_def_interface() {
        let (def, in_id, out_id) = adder_def();
        let view = GraphView::for_graph(&def.graph);
        let mut scene = Scene::default();
        scene.rebuild(
            &def.graph,
            &view,
            &FuncLib::default(),
            Some(&def),
            &HashMap::new(),
        );

        assert_eq!(scene.nodes.len(), 2, "both boundary nodes render");
        let input_node = scene.nodes.iter().find(|n| n.id == in_id).unwrap();
        let output_node = scene.nodes.iter().find(|n| n.id == out_id).unwrap();

        // SubgraphInput: 0 inputs; one output per def *input*, named to
        // match, plus the trailing "+" placeholder.
        assert_eq!(scene.ports(input_node.inputs).len(), 0);
        let in_outs: Vec<&str> = scene
            .ports(input_node.outputs)
            .iter()
            .map(|s| s.as_str(""))
            .collect();
        assert_eq!(in_outs, ["A", "B", "+"]);
        assert!(input_node.subgraph.is_none() && !input_node.terminal);
        assert!(
            input_node.boundary && output_node.boundary,
            "boundary nodes are flagged so const affordances are suppressed"
        );

        // SubgraphOutput: one input per def *output* plus the "+"
        // placeholder; 0 outputs.
        assert_eq!(scene.ports(output_node.outputs).len(), 0);
        let out_ins: Vec<&str> = scene
            .ports(output_node.inputs)
            .iter()
            .map(|s| s.as_str(""))
            .collect();
        assert_eq!(out_ins, ["Sum", "+"]);

        // The interior wire shows up as a connection between the boundaries.
        assert_eq!(scene.connections.len(), 1);
        let c = &scene.connections[0];
        assert_eq!((c.src_node, c.src_port), (in_id, 0));
        assert_eq!((c.tgt_node, c.tgt_port), (out_id, 0));
    }

    #[test]
    fn boundary_nodes_skipped_without_ctx_def() {
        // With no enclosing def (e.g. rendered at the root) a boundary
        // node has no derivable interface, so it's dropped rather than
        // rendered with phantom ports.
        let (def, _in, _out) = adder_def();
        let view = GraphView::for_graph(&def.graph);
        let mut scene = Scene::default();
        scene.rebuild(
            &def.graph,
            &view,
            &FuncLib::default(),
            None,
            &HashMap::new(),
        );

        assert_eq!(scene.nodes.len(), 0, "no ctx_def → no boundary nodes");
        // The wire's endpoints aren't rendered, but `edges()` still yields
        // it; the draw layer skips wires whose ports don't resolve.
        assert_eq!(scene.connections.len(), 1);
    }
}
