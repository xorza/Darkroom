use std::borrow::Cow;
use std::collections::BTreeSet;

use common::Span;
use glam::Vec2;
use palantir::InternedStr;
use scenarium::data::{DataType, StaticValue};
use scenarium::function::{FuncInput, FuncOutput, ValueVariant};
use scenarium::prelude::{
    Binding, FuncLib, Graph, NodeBehavior, NodeId, NodeKind, SubgraphDef, SubgraphRef,
};

use crate::core::document::GraphView;
use crate::gui::run_state::{ExecStatus, RunState};

#[derive(Debug)]
pub struct Scene {
    pub nodes: Vec<SceneNode>,
    pub connections: Vec<SceneConnection>,
    /// One flat pool of [`SceneInput`] across every node, sliced by the single
    /// `SceneNode::inputs` span. A struct-per-port (not parallel columns) so the
    /// per-port fields can't desync. Keeps per-node allocations to zero in
    /// steady state (the `Vec` retains capacity across the per-frame `clear` +
    /// re-`extend`).
    pub inputs: Vec<SceneInput>,
    /// One flat pool of [`SceneOutput`] across every node, sliced by the single
    /// `SceneNode::outputs` span.
    pub outputs: Vec<SceneOutput>,
    /// One flat pool of every input's picker options across all nodes, sliced
    /// per input by [`SceneInput::value_variants`].
    pub value_variants_pool: Vec<ValueVariant>,
    /// Live viewport, mirrored from `Document::{pan, scale}` each
    /// `rebuild`. Read by the canvas transform and the pointer↔world
    /// mapping; the pan/zoom gesture writes it back here, and `App`
    /// copies it onto the document so it persists (single owner =
    /// `Document`).
    pub pan: Vec2,
    pub zoom: f32,
    /// Currently-selected nodes, the committed set mirrored from
    /// `Document` each rebuild so `node_ui` can pick a different paint
    /// without taking a `&Document`. Read-only, like the rest of `Scene`:
    /// the in-progress rubber-band preview lives on `SelectionUI` (read
    /// back via `SelectionUI::preview`) and the canvas unions the two when
    /// drawing, so the gesture never writes into this projection.
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
            inputs: Vec::new(),
            outputs: Vec::new(),
            value_variants_pool: Vec::new(),
            pan: Vec2::ZERO,
            zoom: 1.0,
            selected_nodes: BTreeSet::new(),
        }
    }
}

/// One input port in the per-frame projection. Fields the UI reads together
/// per port (so an AoS pool beats parallel columns here).
#[derive(Debug)]
pub struct SceneInput {
    pub name: InternedStr,
    pub ty: DataType,
    /// Per-frame snapshot of the input's [`Binding`].
    pub binding: InputBindingView,
    /// Default literal (from the func/def interface), resolved once per rebuild
    /// so the UI can offer "set constant" without re-resolving the func lib.
    /// `None` for types with no `StaticValue` (a `Custom` image port).
    pub default: Option<StaticValue>,
    /// A required input with no binding is a missing input — its port renders
    /// highlighted.
    pub required: bool,
    /// Span into [`Scene::value_variants_pool`] for this input's editor picker
    /// options. Empty = no options (the common case).
    pub value_variants: Span,
}

/// One output port in the per-frame projection.
#[derive(Debug)]
pub struct SceneOutput {
    pub name: InternedStr,
    pub ty: DataType,
}

#[derive(Debug)]
pub struct SceneNode {
    pub id: NodeId,
    pub pos: Vec2,
    pub name: InternedStr,
    /// Human-readable type identity: the func name, the subgraph def
    /// name, or the boundary role (`Input`/`Output`). Shown by the
    /// inspection panel.
    pub kind_label: InternedStr,
    /// Span into [`Scene::inputs`].
    pub inputs: Span,
    /// Span into [`Scene::outputs`].
    pub outputs: Span,
    /// `Some` for a composite (`NodeKind::Subgraph`) instance — carries
    /// the ref so the header's open-in-tab action knows which def to
    /// target. `None` for a plain func node.
    pub subgraph: Option<SubgraphRef>,
    /// Sink node (its func is `terminal` — no outputs feed downstream).
    pub terminal: bool,
    /// Result is cached / computed once (`NodeBehavior::Once`). The
    /// header badge toggles this via `Intent::SetCacheBehavior`.
    pub cached: bool,
    /// Excluded from execution (`Node::disabled`). The header badge
    /// toggles this via `Intent::SetDisabled`; the body paints dimmed.
    pub disabled: bool,
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
        run_state: &RunState,
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
        self.inputs.clear();
        self.outputs.clear();
        self.value_variants_pool.clear();

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
                    kind_label: f.name.clone().into(),
                    inputs: Cow::Borrowed(&f.inputs),
                    outputs: Cow::Borrowed(&f.outputs),
                    subgraph: None,
                    terminal: f.terminal,
                }),
                NodeKind::Subgraph(r) => graph.resolve_def(*r, func_lib).map(|d| NodeInterface {
                    kind_label: d.name.clone().into(),
                    inputs: Cow::Borrowed(&d.inputs),
                    outputs: Cow::Borrowed(&d.outputs),
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
                    let mut outputs: Vec<FuncOutput> =
                        d.inputs.iter().map(boundary_output).collect();
                    outputs.push(placeholder_output());
                    NodeInterface {
                        kind_label: "Input".into(),
                        inputs: Cow::Borrowed(&[]),
                        outputs: Cow::Owned(outputs),
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
                        kind_label: "Output".into(),
                        inputs: Cow::Owned(inputs),
                        outputs: Cow::Borrowed(&[]),
                        subgraph: None,
                        terminal: false,
                    }
                }),
            };
            let Some(interface) = interface else {
                continue;
            };
            // One `SceneInput` per input port, sliced by the node's `inputs`
            // span. The bindings come from a parallel graph iterator; each
            // input's value_variants are flattened into one pool, the input
            // recording its span (empty for the common no-options case).
            let inputs_start = self.inputs.len();
            for (input, (_, binding)) in interface
                .inputs
                .iter()
                .zip(graph.node_bindings(node.id, interface.inputs.len()))
            {
                let value_variants = extend_pool(
                    &mut self.value_variants_pool,
                    input.value_variants.iter().cloned(),
                );
                self.inputs.push(SceneInput {
                    name: input.name.clone().into(),
                    ty: input.data_type.clone(),
                    binding: InputBindingView::from(&binding),
                    default: default_static_value(input),
                    required: input.required,
                    value_variants,
                });
            }
            let inputs = Span::new(
                inputs_start as u32,
                (self.inputs.len() - inputs_start) as u32,
            );
            let outputs = extend_pool(
                &mut self.outputs,
                interface.outputs.iter().map(|o| SceneOutput {
                    name: o.name.clone().into(),
                    ty: o.data_type.clone(),
                }),
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
                kind_label: interface.kind_label,
                inputs,
                outputs,
                subgraph: interface.subgraph,
                terminal: interface.terminal,
                cached: node.behavior == NodeBehavior::Once,
                disabled: node.disabled,
                boundary: matches!(
                    node.kind,
                    NodeKind::SubgraphInput | NodeKind::SubgraphOutput
                ),
                exec_status: run_state.status(vn.id),
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

    /// A node's input ports, sliced by its `inputs` span.
    pub fn inputs(&self, span: Span) -> &[SceneInput] {
        slice_pool(&self.inputs, span)
    }

    /// A node's output ports, sliced by its `outputs` span.
    pub fn outputs(&self, span: Span) -> &[SceneOutput] {
        slice_pool(&self.outputs, span)
    }

    /// One input's picker options, resolved from its [`SceneInput::value_variants`]
    /// span into the shared pool.
    pub fn value_variants(&self, span: Span) -> &[ValueVariant] {
        slice_pool(&self.value_variants_pool, span)
    }
}

fn slice_pool<T>(pool: &[T], span: Span) -> &[T] {
    &pool[span.range()]
}

/// View of a node's interface during a single rebuild: the input ports
/// (whole `FuncInput`, for names + defaults) and the output port names.
/// Inputs are usually borrowed from a func/def; the `SubgraphOutput`
/// boundary node synthesizes them from the def's `FuncOutput`s, hence
/// `Cow`.
struct NodeInterface<'a> {
    kind_label: InternedStr,
    inputs: Cow<'a, [FuncInput]>,
    outputs: Cow<'a, [FuncOutput]>,
    subgraph: Option<SubgraphRef>,
    terminal: bool,
}

/// The literal a port falls back to when given a const binding: its
/// declared default, else the zero value for its data type. `None` for a
/// `Custom` type — there is no `StaticValue` for it, so the port can't be
/// given an inline const (and `StaticValue::from` would panic).
fn default_static_value(input: &FuncInput) -> Option<StaticValue> {
    // Explicit default if any, else the type's zero value (`None` for custom
    // types, which have no authorable literal).
    input
        .default_value
        .clone()
        .or_else(|| input.data_type.default_value())
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
        value_variants: Vec::new(),
    }
}

/// Synthesize a `FuncOutput` for a `SubgraphInput`'s output port from the
/// def input it mirrors — name + type carry over.
fn boundary_output(input: &FuncInput) -> FuncOutput {
    FuncOutput {
        name: input.name.clone(),
        data_type: input.data_type.clone(),
    }
}

/// The trailing "connect here to add a port" placeholder output on the
/// `SubgraphInput` boundary node: untyped until something connects.
fn placeholder_output() -> FuncOutput {
    FuncOutput {
        name: PLACEHOLDER_PORT.to_string(),
        data_type: DataType::default(),
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
        value_variants: Vec::new(),
    }
}

fn extend_pool<T>(pool: &mut Vec<T>, items: impl IntoIterator<Item = T>) -> Span {
    let start = pool.len();
    pool.extend(items);
    Span::new(start as u32, (pool.len() - start) as u32)
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
            value_variants: Vec::new(),
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
            origin: None,
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
            &RunState::default(),
        );

        assert_eq!(scene.nodes.len(), 2, "both boundary nodes render");
        let input_node = scene.nodes.iter().find(|n| n.id == in_id).unwrap();
        let output_node = scene.nodes.iter().find(|n| n.id == out_id).unwrap();

        // Boundary nodes are labeled by role.
        assert_eq!(input_node.kind_label.as_str(""), "Input");
        assert_eq!(output_node.kind_label.as_str(""), "Output");

        // SubgraphInput's outputs mirror the def inputs (A:Int, B:Float)
        // plus the untyped "+" placeholder — types align with names.
        let in_outs = scene.outputs(input_node.outputs);
        assert_eq!(in_outs.len(), 3, "two def inputs + placeholder");
        assert!(matches!(in_outs[0].ty, DataType::Int));
        assert!(matches!(in_outs[1].ty, DataType::Float));
        assert!(
            matches!(in_outs[2].ty, DataType::Null),
            "placeholder untyped"
        );

        // SubgraphOutput's inputs mirror the def output (Sum:Int) plus a
        // placeholder.
        let out_ins = scene.inputs(output_node.inputs);
        assert_eq!(out_ins.len(), 2, "one def output + placeholder");
        assert!(matches!(out_ins[0].ty, DataType::Int));

        // SubgraphInput: 0 inputs; one output per def *input*, named to
        // match, plus the trailing "+" placeholder.
        assert_eq!(scene.inputs(input_node.inputs).len(), 0);
        let in_out_names: Vec<&str> = scene
            .outputs(input_node.outputs)
            .iter()
            .map(|o| o.name.as_str(""))
            .collect();
        assert_eq!(in_out_names, ["A", "B", "+"]);
        assert!(input_node.subgraph.is_none() && !input_node.terminal);
        assert!(
            input_node.boundary && output_node.boundary,
            "boundary nodes are flagged so const affordances are suppressed"
        );

        // SubgraphOutput: one input per def *output* plus the "+"
        // placeholder; 0 outputs.
        assert_eq!(scene.outputs(output_node.outputs).len(), 0);
        let out_in_names: Vec<&str> = scene
            .inputs(output_node.inputs)
            .iter()
            .map(|i| i.name.as_str(""))
            .collect();
        assert_eq!(out_in_names, ["Sum", "+"]);

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
            &RunState::default(),
        );

        assert_eq!(scene.nodes.len(), 0, "no ctx_def → no boundary nodes");
        // The wire's endpoints aren't rendered, but `edges()` still yields
        // it; the draw layer skips wires whose ports don't resolve.
        assert_eq!(scene.connections.len(), 1);
    }
}
