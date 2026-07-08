use std::borrow::Cow;
use std::collections::BTreeSet;

use aperture::SmolStr;
use common::{KeyIndexKey, KeyIndexVec, Span};
use glam::Vec2;
use scenarium::data::{DataType, StaticValue};
use scenarium::graph::subgraph::{SubgraphDef, SubgraphRef};
use scenarium::graph::{
    Binding, CachePersistence, Graph, NodeId, NodeKind, OutputPort, Subscription,
};
use scenarium::library::Library;
use scenarium::node::function::{FuncBehavior, FuncInput, FuncOutput, OutputType, ValueVariant};

use crate::core::document::{GraphView, Viewport};
use crate::gui::run_state::{ExecStatus, RunState};

#[derive(Debug, Default)]
pub struct Scene {
    /// Insertion-ordered (draw order) with an `id → index` map, so per-frame
    /// lookups by `NodeId` (`by_key`) are O(1) without losing the ordered
    /// iteration the paint/z-order passes rely on.
    pub nodes: KeyIndexVec<NodeId, SceneNode>,
    pub connections: Vec<SceneConnection>,
    /// Event-subscription edges (emitter event → subscriber node), mirrored
    /// from the active graph each rebuild. Drawn as event wires; the editor
    /// adds/removes them via the subscription drag gesture. The model's
    /// `Subscription` is a plain `Copy` id-bundle with no render-only fields,
    /// so it's mirrored verbatim (unlike `SceneConnection`, which flattens).
    pub subscriptions: Vec<Subscription>,
    /// One flat pool of [`SceneInput`] across every node, sliced by the single
    /// `SceneNode::inputs` span. A struct-per-port (not parallel columns) so the
    /// per-port fields can't desync. Keeps per-node allocations to zero in
    /// steady state (the `Vec` retains capacity across the per-frame `clear` +
    /// re-`extend`).
    pub inputs: Vec<SceneInput>,
    /// One flat pool of [`SceneOutput`] across every node, sliced by the single
    /// `SceneNode::outputs` span.
    pub outputs: Vec<SceneOutput>,
    /// One flat pool of [`SceneEvent`] across every node, sliced by the single
    /// `SceneNode::events` span. Events are emitter ports (always outgoing), so
    /// the UI lists them under the output ports.
    pub events: Vec<SceneEvent>,
    /// One flat pool of every input's picker options across all nodes, sliced
    /// per input by [`SceneInput::value_variants`].
    pub value_variants_pool: Vec<ValueVariant>,
    /// Live viewport, mirrored from the active `GraphView` each `rebuild`.
    /// Read by the canvas transform and the pointer↔world mapping; the
    /// pan/zoom gesture writes it back here, and `App` copies it onto the
    /// document so it persists (single owner = `Document`).
    pub viewport: Viewport,
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

/// One input port in the per-frame projection. Fields the UI reads together
/// per port (so an AoS pool beats parallel columns here).
#[derive(Debug)]
pub struct SceneInput {
    pub name: SmolStr,
    /// Port tooltip from the func's [`FuncInput::description`]; empty when the
    /// port declares none.
    pub description: SmolStr,
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
    /// Const-only inputs reject a wired binding: the connection gesture won't
    /// snap to them, so they can only hold a literal.
    pub const_only: bool,
    /// Span into [`Scene::value_variants_pool`] for this input's editor picker
    /// options. Empty = no options (the common case).
    pub value_variants: Span,
}

/// One output port in the per-frame projection. `ty` is the *resolved* type —
/// for a wildcard output (passthrough / reroute) it's the type followed through
/// the wire (`Any` until something is wired in); the wildcard relationship
/// itself lives on `FuncOutput`'s [`OutputType`], and re-validating downstream
/// wires on an input change is handled at edit time, not from the projection.
#[derive(Debug)]
pub struct SceneOutput {
    pub name: SmolStr,
    /// Port tooltip from the func's [`FuncOutput::description`]; empty when the
    /// port declares none.
    pub description: SmolStr,
    pub ty: DataType,
}

/// One event (emitter) port in the per-frame projection. Events carry no data
/// type — they are pure triggers — so a name is all the UI needs to list them.
#[derive(Debug)]
pub struct SceneEvent {
    pub name: SmolStr,
}

#[derive(Debug)]
pub struct SceneNode {
    pub id: NodeId,
    pub pos: Vec2,
    pub name: SmolStr,
    /// Human-readable type identity: the func name, the subgraph def
    /// name, or the boundary role (`Input`/`Output`). Shown by the
    /// inspection panel.
    pub kind_label: SmolStr,
    /// The func's [`Func::description`] (empty for subgraph/boundary nodes).
    /// Shown by the inspection panel and the new-node palette tooltip.
    pub description: SmolStr,
    /// Span into [`Scene::inputs`].
    pub inputs: Span,
    /// Span into [`Scene::outputs`].
    pub outputs: Span,
    /// Span into [`Scene::events`]. Listed under the output ports.
    pub events: Span,
    /// `Some` for a composite (`NodeKind::Subgraph`) instance — carries
    /// the ref so the header's open-in-tab action knows which def to
    /// target. `None` for a plain func node.
    pub subgraph: Option<SubgraphRef>,
    /// Sink node (its func is `terminal` — no outputs feed downstream).
    pub terminal: bool,
    /// Excluded from execution (`Node::disabled`). The header badge
    /// toggles this via `Intent::SetDisabled`; the body paints dimmed.
    pub disabled: bool,
    /// `true` when this node's output is cached to disk
    /// (`CachePersistence::Disk`), `false` for memory-only. The header `C`
    /// badge toggles it via `Intent::SetPersist`.
    pub persist: bool,
    /// Suppresses the header's `C` (disk-cache/persist) badge. `true` for
    /// self-caching nodes (the file-cache passthrough) and boundary/stub nodes
    /// that have no output to persist.
    pub uncacheable: bool,
    /// The node's func is `Impure`. An impure node has no content digest, so a
    /// `Disk` persist request is silently never honored — the header hides its
    /// `C` badge, like `uncacheable`. `false` for composites and boundary nodes.
    pub impure: bool,
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
    /// The node's func/subgraph def is absent from the library (e.g. a
    /// document saved against an older library), so its interface can't be
    /// resolved. Rendered as a portless error stub the user can still
    /// select and delete — never silently dropped.
    pub missing: bool,
}

impl KeyIndexKey<NodeId> for SceneNode {
    fn key(&self) -> &NodeId {
        &self.id
    }
}

#[derive(Debug)]
pub struct SceneConnection {
    pub src_node: NodeId,
    pub src_port: usize,
    pub tgt_node: NodeId,
    pub tgt_port: usize,
}

impl Scene {
    /// Names live in aperture's per-frame text arena, which clears at
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
        library: &Library,
        ctx_def: Option<&SubgraphDef>,
        run_state: &RunState,
    ) {
        self.selected_nodes = view.selected_nodes.clone();
        // Mirror the persisted viewport. The gesture overwrites this
        // later this frame and `App` copies it back onto the doc, so
        // a fresh value (e.g. just-loaded document) shows up here while
        // an active pan/zoom isn't clobbered (it re-persists each frame).
        self.viewport = view.viewport;
        self.nodes.clear();
        self.connections.clear();
        self.subscriptions.clear();
        self.inputs.clear();
        self.outputs.clear();
        self.events.clear();
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
                NodeKind::Func(func_id) => library.by_id(func_id).map(|f| NodeInterface {
                    kind_label: f.name.clone().into(),
                    description: f.description.clone().unwrap_or_default().into(),
                    inputs: Cow::Borrowed(&f.inputs),
                    outputs: Cow::Borrowed(&f.outputs),
                    events: f.events.iter().map(|e| e.name.clone().into()).collect(),
                    subgraph: None,
                    terminal: f.terminal,
                    uncacheable: f.uncacheable,
                    impure: f.behavior == FuncBehavior::Impure,
                }),
                NodeKind::Subgraph(r) => graph.resolve_def(*r, library).map(|d| NodeInterface {
                    kind_label: d.name.clone().into(),
                    description: "".into(),
                    inputs: Cow::Borrowed(&d.inputs),
                    outputs: Cow::Borrowed(&d.outputs),
                    events: d.events.iter().map(|e| e.name.clone().into()).collect(),
                    subgraph: Some(*r),
                    // A composite's terminal-ness is derived at flatten
                    // time, not stored on the def; treat "no exposed
                    // outputs" as the visible sink signal.
                    terminal: d.outputs.is_empty(),
                    uncacheable: false,
                    // Aggregate purity of a composite isn't known here, so the
                    // persist toggle stays available for it (unlike a func).
                    impure: false,
                }),
                // A built-in special node: its interface is the hardcoded spec.
                // Any wildcard output it declares is resolved below, with every
                // other node's, in one place.
                NodeKind::Special(s) => {
                    let f = s.func();
                    Some(NodeInterface {
                        kind_label: f.name.clone().into(),
                        description: f.description.clone().unwrap_or_default().into(),
                        inputs: Cow::Borrowed(&f.inputs),
                        outputs: Cow::Borrowed(&f.outputs),
                        events: f.events.iter().map(|e| e.name.clone().into()).collect(),
                        subgraph: None,
                        terminal: f.terminal,
                        uncacheable: f.uncacheable,
                        impure: f.behavior == FuncBehavior::Impure,
                    })
                }
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
                        description: "".into(),
                        inputs: Cow::Borrowed(&[]),
                        outputs: Cow::Owned(outputs),
                        events: Vec::new(),
                        subgraph: None,
                        terminal: false,
                        uncacheable: true,
                        impure: false,
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
                        description: "".into(),
                        inputs: Cow::Owned(inputs),
                        outputs: Cow::Borrowed(&[]),
                        events: Vec::new(),
                        subgraph: None,
                        terminal: false,
                        uncacheable: true,
                        impure: false,
                    }
                }),
            };
            // A Func/Subgraph node whose func/def is absent from the library
            // has no interface to project. Dropping it would make it
            // invisible — and so impossible to select and delete — silently
            // corrupting the document. Render it as a portless error stub
            // instead. Boundary nodes only fail to resolve when misplaced at
            // the root (no enclosing `ctx_def`), which still skips.
            let missing = interface.is_none();
            let interface = match interface {
                Some(interface) => interface,
                None => {
                    // Label the stub by what's missing — a func vs a subgraph
                    // def. Boundary nodes only fail to resolve when misplaced
                    // at the root (no enclosing `ctx_def`); nothing to render.
                    let kind_label = match node.kind {
                        NodeKind::Func(_) => "missing func",
                        NodeKind::Subgraph(_) => "missing subgraph",
                        // A special node's spec always resolves, so it never
                        // reaches this `None` branch; boundary nodes only fail at
                        // the root. Nothing to render either way.
                        NodeKind::Special(_)
                        | NodeKind::SubgraphInput
                        | NodeKind::SubgraphOutput => continue,
                    };
                    NodeInterface {
                        kind_label: kind_label.into(),
                        description: "".into(),
                        inputs: Cow::Borrowed(&[]),
                        outputs: Cow::Borrowed(&[]),
                        events: Vec::new(),
                        subgraph: None,
                        terminal: false,
                        uncacheable: true,
                        impure: false,
                    }
                }
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
                    description: input.description.clone().unwrap_or_default().into(),
                    ty: input.data_type.clone(),
                    binding: InputBindingView::from(&binding),
                    default: default_static_value(library, input),
                    required: input.required,
                    const_only: input.const_only,
                    value_variants,
                });
            }
            let inputs = Span::new(
                inputs_start as u32,
                (self.inputs.len() - inputs_start) as u32,
            );
            let outputs = extend_pool(
                &mut self.outputs,
                interface
                    .outputs
                    .iter()
                    .enumerate()
                    .map(|(i, o)| SceneOutput {
                        name: o.name.clone().into(),
                        description: o.description.clone().unwrap_or_default().into(),
                        // A wildcard output (passthrough / reroute) reports the type
                        // resolved through the input it mirrors; a fixed output uses
                        // its declared type.
                        ty: match &o.ty {
                            OutputType::Wildcard { .. } => {
                                graph.resolve_output_type(library, OutputPort::new(node.id, i))
                            }
                            OutputType::Fixed(dt) => dt.clone(),
                        },
                    }),
            );
            let events = extend_pool(
                &mut self.events,
                interface
                    .events
                    .iter()
                    .map(|name| SceneEvent { name: name.clone() }),
            );
            // Boundary nodes carry no name in the model; label them by
            // role so the interior header isn't a blank bar.
            let name: SmolStr = match (&node.kind, node.name.is_empty()) {
                (NodeKind::SubgraphInput, true) => "Inputs".into(),
                (NodeKind::SubgraphOutput, true) => "Outputs".into(),
                _ => node.name.clone().into(),
            };
            self.nodes.add(SceneNode {
                id: vn.id,
                pos: vn.pos,
                name,
                kind_label: interface.kind_label,
                description: interface.description,
                inputs,
                outputs,
                events,
                subgraph: interface.subgraph,
                terminal: interface.terminal,
                disabled: node.disabled,
                persist: node.persist == CachePersistence::Disk,
                uncacheable: interface.uncacheable,
                impure: interface.impure,
                boundary: matches!(
                    node.kind,
                    NodeKind::SubgraphInput | NodeKind::SubgraphOutput
                ),
                exec_status: run_state.status(vn.id),
                missing,
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

        self.subscriptions.extend(graph.subscriptions());
    }

    /// A node's input ports, sliced by its `inputs` span.
    pub fn inputs(&self, span: Span) -> &[SceneInput] {
        slice_pool(&self.inputs, span)
    }

    /// A node's output ports, sliced by its `outputs` span.
    pub fn outputs(&self, span: Span) -> &[SceneOutput] {
        slice_pool(&self.outputs, span)
    }

    /// A node's event (emitter) ports, sliced by its `events` span.
    pub fn events(&self, span: Span) -> &[SceneEvent] {
        slice_pool(&self.events, span)
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
    kind_label: SmolStr,
    /// The func's description (empty for subgraphs/boundaries/missing stubs).
    description: SmolStr,
    inputs: Cow<'a, [FuncInput]>,
    outputs: Cow<'a, [FuncOutput]>,
    /// Event (emitter) port names, in declaration order. `FuncEvent` and
    /// `SubgraphEvent` differ in type but both expose a `name`, and the UI
    /// only lists the name — so the interface flattens them to owned
    /// names rather than threading a third `Cow<[_]>` of incompatible types.
    events: Vec<SmolStr>,
    subgraph: Option<SubgraphRef>,
    terminal: bool,
    /// Node manages its own caching (or has no output to cache), so the editor's
    /// disk-cache (persist) toggle is hidden — see [`SceneNode::uncacheable`].
    uncacheable: bool,
    /// The func is `Impure`, so it has no content digest and a `Disk` request is
    /// silently never honored — the editor hides its persist toggle (see
    /// [`SceneNode::impure`]). Only set from a func spec; `false` for composites
    /// (aggregate purity isn't known here) and boundary/stub nodes.
    impure: bool,
}

/// The literal a port falls back to when given a const binding: its declared
/// default, else the zero value for its data type. `None` for a `Custom` type —
/// there is no `StaticValue` for it, so the port can't be given an inline const.
fn default_static_value(library: &Library, input: &FuncInput) -> Option<StaticValue> {
    input.default_value.clone().or_else(|| {
        // An enum's first-variant default needs the library's registered variant
        // list — the bare `DataType::Enum(id)` doesn't carry it, so resolve it
        // here so an enum port gets the same const affordance as a scalar.
        match &input.data_type {
            DataType::Enum(id) => library
                .enum_variants(id)
                .and_then(|variants| variants.first())
                .map(|first| StaticValue::Enum(first.clone())),
            // An untyped (`Any`) port has no concrete kind to seed; start it as
            // an empty string so the smart editor opens blank and infers the
            // kind from whatever the user types (see `value_editor::parse_any`).
            DataType::Any => Some(StaticValue::String(String::new())),
            ty => ty.default_value(),
        }
    })
}

/// Synthesize a `FuncInput` for a `SubgraphOutput`'s input port from the
/// def output it mirrors — name + type carry over; it's not user-set, so
/// it has no declared default and no value options.
fn boundary_input(output: &FuncOutput) -> FuncInput {
    FuncInput::optional(output.name.clone(), output.ty.declared())
}

/// Synthesize a `FuncOutput` for a `SubgraphInput`'s output port from the
/// def input it mirrors — name + type carry over.
fn boundary_output(input: &FuncInput) -> FuncOutput {
    FuncOutput::new(input.name.clone(), input.data_type.clone())
}

/// The trailing "connect here to add a port" placeholder output on the
/// `SubgraphInput` boundary node: untyped until something connects.
fn placeholder_output() -> FuncOutput {
    FuncOutput::new(PLACEHOLDER_PORT, DataType::default())
}

/// Label for the trailing "connect here to add a port" placeholder on a
/// boundary node.
const PLACEHOLDER_PORT: &str = "+";

/// The placeholder input port for a `SubgraphOutput`: unbound, untyped
/// until something connects (at which point reconcile materializes a real
/// `def.outputs` entry from the wired producer's type).
fn placeholder_input() -> FuncInput {
    FuncInput::optional(PLACEHOLDER_PORT, DataType::default())
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
    use scenarium::graph::subgraph::SubgraphDef;
    use scenarium::graph::{InputPort, Node};

    fn finput(name: &str, ty: DataType) -> FuncInput {
        FuncInput::optional(name, ty)
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

        let def = SubgraphDef::new("00000000-0000-0000-0000-000000000000", "Adder")
            .category("Subgraph")
            .graph(inner)
            .inputs([finput("A", DataType::Int), finput("B", DataType::Float)])
            .output(FuncOutput::new("Sum", DataType::Int));
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
            &Library::default(),
            Some(&def),
            &RunState::default(),
        );

        assert_eq!(scene.nodes.len(), 2, "both boundary nodes render");
        let input_node = scene.nodes.iter().find(|n| n.id == in_id).unwrap();
        let output_node = scene.nodes.iter().find(|n| n.id == out_id).unwrap();

        // Boundary nodes are labeled by role.
        assert_eq!(input_node.kind_label.as_str(), "Input");
        assert_eq!(output_node.kind_label.as_str(), "Output");

        // SubgraphInput's outputs mirror the def inputs (A:Int, B:Float)
        // plus the untyped "+" placeholder — types align with names.
        let in_outs = scene.outputs(input_node.outputs);
        assert_eq!(in_outs.len(), 3, "two def inputs + placeholder");
        assert!(matches!(in_outs[0].ty, DataType::Int));
        assert!(matches!(in_outs[1].ty, DataType::Float));
        assert!(
            matches!(in_outs[2].ty, DataType::Any),
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
            .map(|o| o.name.as_str())
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
            .map(|i| i.name.as_str())
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
            &Library::default(),
            None,
            &RunState::default(),
        );

        assert_eq!(scene.nodes.len(), 0, "no ctx_def → no boundary nodes");
        // The wire's endpoints aren't rendered, but `edges()` still yields
        // it; the draw layer skips wires whose ports don't resolve.
        assert_eq!(scene.connections.len(), 1);
    }

    #[test]
    fn missing_func_and_subgraph_render_as_deletable_stubs() {
        use scenarium::elements::math_library::math_library;
        use scenarium::graph::subgraph::SubgraphRef;

        // A resolvable func, plus two unresolvable nodes (e.g. a document
        // saved against an older library): a func id and a linked subgraph
        // def id the library no longer defines.
        let library = math_library();
        let mut graph = Graph::default();
        let known: Node = library.by_name("Add").unwrap().into();
        let known_id = known.id;
        let mut ghost_func = Node::new(NodeKind::Func(
            "7a0265e1-9631-45bd-8ecd-1e923b67a58c".into(),
        ));
        ghost_func.name = "astro_to_image".into();
        let ghost_func_id = ghost_func.id;
        let mut ghost_sub = Node::new(NodeKind::Subgraph(SubgraphRef::Linked(
            "00000000-0000-0000-0000-0000000000ff".into(),
        )));
        ghost_sub.name = "removed_subgraph".into();
        let ghost_sub_id = ghost_sub.id;
        graph.add(known);
        graph.add(ghost_func);
        graph.add(ghost_sub);

        let view = GraphView::for_graph(&graph);
        let mut scene = Scene::default();
        scene.rebuild(&graph, &view, &library, None, &RunState::default());

        // Every node renders, not silently dropped — so the unresolvable ones
        // stay selectable and deletable to repair the document.
        assert_eq!(scene.nodes.len(), 3, "all nodes render");
        let known_node = scene.nodes.iter().find(|n| n.id == known_id).unwrap();
        let ghost_func_node = scene.nodes.iter().find(|n| n.id == ghost_func_id).unwrap();
        let ghost_sub_node = scene.nodes.iter().find(|n| n.id == ghost_sub_id).unwrap();

        // The flag tracks resolution; the label names what's missing.
        assert!(!known_node.missing, "a resolved func is not a stub");
        assert!(ghost_func_node.missing && ghost_sub_node.missing);
        assert_eq!(ghost_func_node.kind_label.as_str(), "missing func");
        assert_eq!(ghost_sub_node.kind_label.as_str(), "missing subgraph");

        // Both stubs keep their saved name and carry no ports — and the
        // subgraph stub drops its `subgraph` ref so the "open in tab" action
        // isn't offered for a def that isn't there.
        assert_eq!(ghost_func_node.name.as_str(), "astro_to_image");
        assert_eq!(ghost_sub_node.name.as_str(), "removed_subgraph");
        assert!(ghost_sub_node.subgraph.is_none());
        for stub in [ghost_func_node, ghost_sub_node] {
            assert_eq!(scene.inputs(stub.inputs).len(), 0, "stub has no inputs");
            assert_eq!(scene.outputs(stub.outputs).len(), 0, "stub has no outputs");
        }

        // The resolved node, by contrast, exposes its real ports.
        assert!(
            !scene.inputs(known_node.inputs).is_empty(),
            "the resolved func still renders its interface"
        );
    }

    #[test]
    fn func_events_project_in_order_alongside_outputs() {
        use scenarium::elements::worker_events_library::{
            FRAME_EVENT_FUNC_ID, worker_events_library,
        };

        // The `frame event` func declares two events ("Always", "FPS") and two
        // data outputs ("Delta", "Frame #"); the projection must surface both
        // independently — events in their own pool, outputs unchanged.
        let library = worker_events_library();
        let mut graph = Graph::default();
        let node: Node = library.by_id(&FRAME_EVENT_FUNC_ID).unwrap().into();
        let node_id = node.id;
        graph.add(node);

        let view = GraphView::for_graph(&graph);
        let mut scene = Scene::default();
        scene.rebuild(&graph, &view, &library, None, &RunState::default());

        let n = scene.nodes.iter().find(|n| n.id == node_id).unwrap();
        let event_names: Vec<&str> = scene
            .events(n.events)
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(event_names, ["Always", "FPS"], "events project in order");

        let output_names: Vec<&str> = scene
            .outputs(n.outputs)
            .iter()
            .map(|o| o.name.as_str())
            .collect();
        assert_eq!(
            output_names,
            ["Delta", "Frame #"],
            "data outputs are unaffected by events"
        );
    }

    #[test]
    fn subscriptions_project_from_graph() {
        use scenarium::elements::worker_events_library::{
            FRAME_EVENT_FUNC_ID, worker_events_library,
        };

        // Two frame-event nodes; subscribe the second to the first's "FPS"
        // event (event_idx 1). The projection must mirror that one edge.
        let library = worker_events_library();
        let mut graph = Graph::default();
        let emitter: Node = library.by_id(&FRAME_EVENT_FUNC_ID).unwrap().into();
        let emitter_id = emitter.id;
        graph.add(emitter);
        let subscriber: Node = library.by_id(&FRAME_EVENT_FUNC_ID).unwrap().into();
        let subscriber_id = subscriber.id;
        graph.add(subscriber);
        graph.subscribe(emitter_id, 1, subscriber_id);

        let view = GraphView::for_graph(&graph);
        let mut scene = Scene::default();
        scene.rebuild(&graph, &view, &library, None, &RunState::default());

        assert_eq!(scene.subscriptions.len(), 1);
        let s = &scene.subscriptions[0];
        assert_eq!(s.emitter, emitter_id);
        assert_eq!(s.event_idx, 1);
        assert_eq!(s.subscriber, subscriber_id);
    }

    #[test]
    fn persist_flag_projects_disk_as_true_memory_as_false() {
        use scenarium::elements::math_library::math_library;
        use scenarium::graph::CachePersistence;

        // Two identical funcs differing only in cache policy: one default
        // (Memory), one Disk. The projection must mirror each.
        let library = math_library();
        let mut graph = Graph::default();
        let memory_node: Node = library.by_name("Add").unwrap().into();
        let memory_id = memory_node.id;
        graph.add(memory_node);
        let mut disk_node: Node = library.by_name("Add").unwrap().into();
        disk_node.persist = CachePersistence::Disk;
        let disk_id = disk_node.id;
        graph.add(disk_node);

        let view = GraphView::for_graph(&graph);
        let mut scene = Scene::default();
        scene.rebuild(&graph, &view, &library, None, &RunState::default());

        let memory = scene.nodes.iter().find(|n| n.id == memory_id).unwrap();
        let disk = scene.nodes.iter().find(|n| n.id == disk_id).unwrap();
        assert!(!memory.persist, "default node projects memory-only");
        assert!(disk.persist, "Disk-marked node projects persist=true");
    }

    #[test]
    fn impure_flag_projects_from_func_behavior() {
        use scenarium::node::function::Func;

        // Two funcs identical but for behavior: a `Pure` one (offers the disk-cache
        // toggle) and an `Impure` one (has no content digest, so the toggle is hidden).
        // Both have an output and are non-terminal/cacheable, so `impure` is the sole
        // differentiator the header gate reads.
        let mut library = Library::default();
        library.add(
            Func::new("bbebd119-82d8-45cc-a710-cdaa45426521", "pure_src")
                .pure()
                .output(FuncOutput::new("out", DataType::Int)),
        );
        library.add(
            Func::new("9a97bb06-2c2e-443a-a836-6a11e29cbea7", "impure_src")
                .output(FuncOutput::new("out", DataType::Int)),
        );

        let mut graph = Graph::default();
        let pure_id = graph.add_func_node(library.by_name("pure_src").unwrap());
        let impure_id = graph.add_func_node(library.by_name("impure_src").unwrap());

        let view = GraphView::for_graph(&graph);
        let mut scene = Scene::default();
        scene.rebuild(&graph, &view, &library, None, &RunState::default());

        let pure = scene.nodes.iter().find(|n| n.id == pure_id).unwrap();
        let impure = scene.nodes.iter().find(|n| n.id == impure_id).unwrap();

        assert!(!pure.impure, "a Pure func keeps its persist toggle");
        assert!(impure.impure, "an Impure func hides its persist toggle");
        // Isolate `impure` as the cause: neither is self-caching or a terminal sink.
        assert!(!pure.uncacheable && !pure.terminal);
        assert!(!impure.uncacheable && !impure.terminal);
    }
}
