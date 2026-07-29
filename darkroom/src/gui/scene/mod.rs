use std::borrow::Cow;
use std::collections::BTreeSet;

use common::Span;
use glam::Vec2;
use indexmap::IndexMap;
use palantir::{InternedStr, Ui};
use scenarium::Library;
use scenarium::{
    Binding, CacheMode, Graph, InputPort, Node, NodeId, NodeKind, NodeSearch, OutputPort,
    Subscription,
};
use scenarium::{DataType, GraphDef, GraphInterface, NodeEvents, RamUsage, StaticValue};
use scenarium::{FuncBehavior, FuncInput, FuncOutput, OutputType, ValueVariant};
use scenarium::{GraphId, GraphLink};

use crate::core::document::{GraphRef, GraphView, PortKind, PortRef, Viewport};
use crate::core::preview;
use crate::gui::EventRef;
use crate::gui::run_state::{ExecStatus, RunState};

/// The per-record projection of **every graph currently on screen** — one
/// entry in `graphs` per visible graph pane.
///
/// Everything a graph owns lives in a pool shared with all the others and
/// is sliced by a [`Span`] on its [`SceneGraph`]: paint stack, node
/// projections, wiring, subscriptions, selection, and the per-port pools
/// under those. Two panes therefore cost one set of allocations, not two,
/// and the steady-state rebuild still allocates nothing (every `Vec`
/// retains capacity across the per-frame `clear` + re-`extend`).
///
/// Reads go through [`GraphScene`], a `Copy` borrow pairing this with one
/// of its graphs — `scene.graph(target)` for a named pane,
/// `scene.owner(node_id)` for whichever pane a node sits in.
#[derive(Debug, Default)]
pub(crate) struct Scene {
    /// One projected graph per visible graph pane, in dock-pane order.
    /// Every field below is a pool this slices into.
    graphs: IndexMap<GraphRef, SceneGraph>,
    /// Each graph's paint stack, mirrored from its `GraphView::item_placements`
    /// order: later entries drawn in front. The canvas draw pass iterates its
    /// own graph's slice; everything else looks items up through `nodes`.
    z_order: Vec<NodeId>,
    /// Keyed node projections in relative paint order, **spanning every
    /// projected graph**. Node ids are unique across the whole document
    /// (upheld by `Graph::clone_mapped` at every copy boundary and enforced
    /// by `Document::validate`), so one map can hold them all without
    /// collision: a by-id lookup stays a single hash however many panes are
    /// open, and `SceneNode::owner` names the graph a hit belongs to.
    /// Interaction scans use this order to resolve overlapping node and port
    /// hits.
    pub(crate) nodes: IndexMap<NodeId, SceneNode>,
    connections: Vec<SceneConnection>,
    /// Event-subscription edges (emitter event → subscriber node), mirrored
    /// from each graph each rebuild. Drawn as event wires; the editor
    /// adds/removes them via the subscription drag gesture. The model's
    /// `Subscription` is a plain `Copy` id-bundle with no render-only fields,
    /// so it's mirrored verbatim (unlike `SceneConnection`, which flattens).
    subscriptions: Vec<Subscription>,
    /// Currently-selected nodes, mirrored from each `GraphView` each rebuild
    /// so `node_ui` can pick a different paint without taking a `&Document`. Sorted within each
    /// graph's span (`GraphView::selected` is a `BTreeSet`, so extending
    /// preserves it), which is what makes [`GraphScene::is_selected`] a
    /// binary search rather than a set lookup. Read-only, like the rest of
    /// `Scene`: the in-progress rubber-band preview lives on `SelectionUI`
    /// (read back via `SelectionUI::preview`) and the canvas unions the two
    /// when drawing, so the gesture never writes into this projection.
    selected: Vec<NodeId>,
    /// One flat pool of [`SceneInput`] across every node of every graph,
    /// sliced by the single `SceneNode::inputs` span. A struct-per-port (not
    /// parallel columns) so the per-port fields can't desync.
    inputs: Vec<SceneInput>,
    /// One flat pool of [`SceneOutput`], sliced by `SceneNode::outputs`.
    outputs: Vec<SceneOutput>,
    /// One flat pool of [`SceneEvent`], sliced by `SceneNode::events`.
    /// Events are emitter ports (always outgoing), so the UI lists them
    /// under the output ports.
    events: Vec<SceneEvent>,
    /// One flat pool of every input's picker options, sliced per input by
    /// [`SceneInput::value_variants`].
    value_variants_pool: Vec<ValueVariant>,
    /// Each graph's own local definitions, sliced by
    /// [`SceneGraph::local_defs`]. Only the palette reads these — a pane's
    /// defs are what its "add node" popup can instance without materializing
    /// a copy.
    local_defs: Vec<SceneLocalDef>,
}

/// One local [`GraphDef`] held by a projected graph — enough to list it in
/// the palette and raise the intent that instances it, no more. The
/// definition itself stays in the document: `build_step` resolves the id
/// against it, so the projection never carries an interface.
#[derive(Debug)]
pub(crate) struct SceneLocalDef {
    pub(crate) id: GraphId,
    pub(crate) name: InternedStr,
    pub(crate) category: InternedStr,
    /// Library entry this definition was copied from, if any. The palette
    /// drops the library's own row for it: clicking either one instances
    /// *this* definition (`build::local_graph_from`), so two rows would
    /// offer one outcome under one name.
    pub(crate) origin: Option<GraphId>,
}

/// One projected graph: `Span`s into [`Scene`]'s pools plus the small
/// per-graph view state the canvas reads directly. No owned collections, so
/// opening a second graph pane costs nothing beyond the pools growing.
#[derive(Debug)]
pub(crate) struct SceneGraph {
    /// Which graph this pane shows — the target every intent raised over it
    /// is committed against.
    target: GraphRef,
    /// Live viewport, mirrored from this graph's `GraphView` each
    /// `rebuild`. Read by the canvas transform and the pointer↔world
    /// mapping; the pan/zoom gesture writes it back here, and `App` copies
    /// it onto the document so it persists (single owner = `Document`).
    viewport: Viewport,
    /// Slice of [`Scene::nodes`] holding exactly this graph's nodes. The
    /// projection fills one graph at a time, so each graph's nodes are
    /// contiguous.
    nodes: Span,
    z_order: Span,
    connections: Span,
    subscriptions: Span,
    selected: Span,
    /// Slice of [`Scene::local_defs`] holding this graph's own local
    /// definitions — the ones a `GraphLink::Local` raised over this pane can
    /// resolve (`validate::insertable_kind` accepts no others).
    local_defs: Span,
    /// Whether a run raised over this pane resolves to one occurrence. False
    /// in a definition pane, which is no particular instance of that
    /// definition — so a run there has no single occurrence to target.
    ///
    /// A fact about the *pane*, so it lives here rather than being copied
    /// onto each of its nodes; readers reach it through
    /// [`GraphScene::runnable`], which is always called with the pane in hand
    /// anyway.
    run_available: bool,
}

/// One graph to project this record pass: which target it is, where its
/// nodes and wiring come from, and the view metadata positioning them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GraphProjection<'a> {
    pub(crate) target: GraphRef,
    pub(crate) source: SceneSource<'a>,
    pub(crate) view: &'a GraphView,
}

/// A [`Scene`] borrowed together with one of its graphs — the read handle
/// every per-pane widget takes instead of a bare `&Scene`. `Copy` (two
/// shared refs), so it threads through the draw chain like `RecordCtx`.
///
/// Anything scoped to one pane (its paint stack, wiring, selection,
/// viewport) reads through here; the whole-scene sweeps that are keyed by
/// document-unique ids — `CanvasGeometry`'s rebuild, a drag's
/// owner-still-alive check — take the `scene` field directly.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GraphScene<'a> {
    pub(crate) scene: &'a Scene,
    graph: &'a SceneGraph,
}

/// Per-frame snapshot of an input port's [`Binding`] for the UI tree.
/// Variant-only for `Bind`; the address details live on the graph's
/// connection slice.
#[derive(Debug, Clone)]
pub(crate) enum InputBindingView {
    None,
    Const(StaticValue),
    Bind,
}

impl From<Option<&Binding>> for InputBindingView {
    fn from(binding: Option<&Binding>) -> Self {
        match binding {
            None => Self::None,
            Some(Binding::Const(value)) => Self::Const(value.clone()),
            Some(Binding::Bind(_)) => Self::Bind,
        }
    }
}

/// One input port in the per-frame projection. Fields the UI reads together
/// per port (so an AoS pool beats parallel columns here).
#[derive(Debug)]
pub(crate) struct SceneInput {
    pub(crate) name: InternedStr,
    /// Port tooltip from the func's [`FuncInput::description`]; empty when the
    /// port declares none.
    pub(crate) description: InternedStr,
    pub(crate) ty: DataType,
    /// Per-frame snapshot of the input's [`Binding`].
    pub(crate) binding: InputBindingView,
    /// Default literal (from the func/graph interface), resolved once per rebuild
    /// so the UI can offer "set constant" without re-resolving the func lib.
    /// `None` for types with no `StaticValue` (a `Custom` image port).
    pub(crate) default: Option<StaticValue>,
    /// Required inputs render with more visual weight than optional ones.
    pub(crate) required: bool,
    /// The last run could not feed this port — its own verdict, so a port wired to a
    /// disabled or itself-unfed producer counts too, not just an unbound one. Renders
    /// highlighted while the node reads `MissingInputs`.
    pub(crate) missing: bool,
    /// Const-only inputs reject a wired binding: the connection gesture won't
    /// snap to them, so they can only hold a literal.
    pub(crate) const_only: bool,
    /// Span into [`Scene::value_variants_pool`] for this input's editor picker
    /// options. Empty = no options (the common case).
    pub(crate) value_variants: Span,
}

/// One output port in the per-frame projection. `ty` is the *resolved* type —
/// for a wildcard output (passthrough / reroute) it's the type followed through
/// the wire (`Any` until something is wired in); the wildcard relationship
/// itself lives on `FuncOutput`'s [`OutputType`], and re-validating downstream
/// wires on an input change is handled at edit time, not from the projection.
#[derive(Debug)]
pub(crate) struct SceneOutput {
    pub(crate) name: InternedStr,
    /// Port tooltip from the func's [`FuncOutput::description`]; empty when the
    /// port declares none.
    pub(crate) description: InternedStr,
    pub(crate) ty: DataType,
}

/// One event (emitter) port in the per-frame projection. Events carry no data
/// type — they are pure triggers — so a name is all the UI needs to list them.
#[derive(Debug)]
pub(crate) struct SceneEvent {
    pub(crate) name: InternedStr,
}

#[derive(Debug)]
pub(crate) struct SceneNode {
    pub(crate) id: NodeId,
    /// Which projected graph this node belongs to — how a whole-scene scan
    /// (a port drag, a chip click, a context menu) routes the intent it
    /// raises to the right editing target now that several graphs share one
    /// projection.
    pub(crate) owner: GraphRef,
    pub(crate) pos: Vec2,
    pub(crate) name: InternedStr,
    /// Human-readable type identity: the func name, the graph
    /// name, or the boundary role (`Input`/`Output`). Shown by the
    /// inspection panel.
    pub(crate) kind_label: InternedStr,
    /// The func's [`Func::description`](scenarium::Func::description) (empty for graph/boundary nodes).
    /// Shown by the inspection panel and the new-node palette tooltip.
    pub(crate) description: InternedStr,
    /// Span into [`Scene::inputs`].
    pub(crate) inputs: Span,
    /// Span into [`Scene::outputs`].
    pub(crate) outputs: Span,
    /// Span into [`Scene::events`]. Listed under the output ports.
    pub(crate) events: Span,
    /// `Some` for a composite (`NodeKind::Graph`) instance — carries
    /// the ref so the header's open-in-tab action knows which graph to
    /// target. `None` for a plain func node.
    pub(crate) graph: Option<GraphLink>,
    /// Sink node (its func is `sink` — no outputs feed downstream).
    pub(crate) sink: bool,
    /// Excluded from execution (`Node::disabled`). Sink headers expose the
    /// toggle; the body paints any authored disabled node dimmed.
    pub(crate) disabled: bool,
    /// Where this node's output is cached ([`CacheMode`]). The header's two storage
    /// chips toggle its RAM and disk bits.
    pub(crate) cache: CacheMode,
    /// Whether this node has an executable slot whose RAM/disk storage policy can
    /// be changed directly.
    pub(crate) cache_controls: bool,
    /// Whether the header offers runtime cache eviction for this node. A graph
    /// instance evicts its flattened interior; a func needs a reproducible
    /// output. Boundary and impure nodes have no reusable output to evict.
    pub(crate) can_evict_cache: bool,
    /// The node holds work that recomputes every run. An impure node has no
    /// content digest, so no cache mode is ever honored (folded into the cache
    /// controls); the header paints the `~` marker off this flag.
    ///
    /// A composite inherits it from its interior — one impure node in there is
    /// enough — so the marker reads "this isn't reusable", not "this is why
    /// there are no storage chips". A composite has no chips for its own
    /// reason ([`NodeInterface::cache_controls`]: its storage is the
    /// interior's business), impure or not.
    pub(crate) impure: bool,
    /// A `GraphInput`/`GraphOutput` interface boundary node. Its
    /// ports route the graph interface rather than carry literal
    /// values, so the const-value affordances (inline editor, "Set
    /// constant" menu, drag-to-own-body) are suppressed on them.
    pub(crate) boundary: bool,
    /// Outcome of the last graph run, mirrored from `WorkerStatus`. Drives the
    /// node's status-glow shadow and (for
    /// `Executed`) the header time label; `None` (the default) paints
    /// no glow.
    pub(crate) exec_status: ExecStatus,
    /// A preview node: its body shows the value wired into it instead of the
    /// usual output ports and memory readout. Resolved from the func id, so a
    /// document whose library lost the func degrades to an ordinary missing
    /// stub rather than an empty card.
    pub(crate) preview: bool,
    /// RAM this node's cached output currently holds (system vs GPU), mirrored
    /// from `run_state`. Non-zero only for nodes that retain a value; drives the
    /// node body's memory readout, hidden when zero.
    pub(crate) ram: RamUsage,
    /// The node's func/graph is absent from the library (e.g. a
    /// document saved against an older library), so its interface can't be
    /// resolved. Rendered as a portless error stub the user can still
    /// select and delete — never silently dropped.
    pub(crate) missing: bool,
}

impl SceneNode {
    /// Every `PortRef` on the given side, in port order. Single source for
    /// the "iterate a node's ports by kind" loop `CanvasGeometry::rebuild`
    /// and the connection scans all need, so scan order and paint order
    /// can't drift apart.
    pub(crate) fn ports(&self, kind: PortKind) -> impl Iterator<Item = PortRef> + '_ {
        let span = match kind {
            PortKind::Input => self.inputs,
            PortKind::Output => self.outputs,
        };
        (0..span.len as usize).map(move |port_idx| PortRef {
            node_id: self.id,
            kind,
            port_idx,
        })
    }

    /// Every `EventRef`, in declaration order — the emitter-glyph
    /// counterpart of [`Self::ports`], shared by `CanvasGeometry::rebuild`
    /// and the subscription-wire scans for the same reason.
    pub(crate) fn events(&self) -> impl Iterator<Item = EventRef> + '_ {
        (0..self.events.len as usize).map(|event_idx| EventRef {
            node_id: self.id,
            event_idx,
        })
    }

    /// Whether this node covers any compiled work at all. A boundary node is
    /// wiring — flatten resolves *through* it and emits nothing — and a
    /// `missing` stub resolves to nothing. Everything else does, graph
    /// instances included: a composite dissolves into its interior rather
    /// than vanishing, which is what
    /// [`CompiledGraph::run_targets`](scenarium::CompiledGraph::run_targets)
    /// resolves it to.
    ///
    /// Deliberately an authoring-side fact, not a lookup in a compiled
    /// program: the palette and the header record every frame, including
    /// before the first compile, so an affordance can't wait on one.
    pub(crate) fn executable_kind(&self) -> bool {
        !self.boundary && !self.missing
    }

    /// Whether Darkroom exposes the disable toggle for this node. Limiting it
    /// to runnable sinks keeps disabled nodes directly runnable with their
    /// upstream cone intact.
    pub(crate) fn can_disable(&self) -> bool {
        self.sink && self.executable_kind()
    }
}

#[derive(Debug)]
pub(crate) struct SceneConnection {
    /// Producer side — the output feeding the wire.
    pub(crate) src: OutputPort,
    /// Consumer side — the input the wire lands on.
    pub(crate) tgt: InputPort,
}

/// What one graph's rebuild projects. An enum rather than a graph + optional
/// interface, so a definition body separated from its interface — which
/// would strand its boundary nodes with nothing to mirror — can't be
/// spelled at all.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SceneSource<'a> {
    /// The document root: no interface, and no boundary nodes to want one
    /// (`validate_for_execution` rejects them there).
    Entry(&'a Graph),
    /// A local definition: the body on the canvas, plus the interface its
    /// boundary nodes mirror.
    Def(&'a GraphDef),
}

impl<'a> SceneSource<'a> {
    /// The graph whose nodes and wiring the scene projects.
    pub(crate) fn graph(self) -> &'a Graph {
        match self {
            SceneSource::Entry(graph) => graph,
            SceneSource::Def(def) => &def.body,
        }
    }

    /// The interface a boundary node mirrors, if this source has one.
    pub(crate) fn interface(self) -> Option<&'a GraphInterface> {
        match self {
            SceneSource::Entry(_) => None,
            SceneSource::Def(def) => Some(&def.interface),
        }
    }
}

impl Scene {
    /// Project every graph in `projections`, replacing the previous pass's
    /// contents.
    ///
    /// Names are arena-backed handles authored through this record pass's
    /// `Ui`. Rebuilding keeps the projection synchronized with the document
    /// and lets the previous pass's text arena be recycled. `App::record`
    /// enforces this before widgets consume the scene.
    pub(crate) fn rebuild<'a>(
        &mut self,
        ui: &mut Ui,
        library: &Library,
        run_state: &RunState,
        projections: impl IntoIterator<Item = GraphProjection<'a>>,
    ) {
        self.clear_pools();

        // One handle for the empty string, cloned wherever a port declares no
        // description or a node carries no authored name — see
        // `intern_or_empty`.
        let empty = ui.intern("");
        for projection in projections {
            let graph = self.project(ui, library, run_state, projection, &empty);
            self.graphs.insert(projection.target, graph);
        }
    }

    /// Empty every pool, keeping its capacity, so the projection can be
    /// refilled from scratch.
    ///
    /// Destructured rather than a run of `self.x.clear()` lines: a pool added
    /// to [`Scene`] without a matching clear here would carry last frame's
    /// contents into the next pass's spans, and the `let` without `..` turns
    /// that from a silent bug into a compile error naming the missing field.
    fn clear_pools(&mut self) {
        let Self {
            graphs,
            z_order,
            nodes,
            connections,
            subscriptions,
            selected,
            inputs,
            outputs,
            events,
            value_variants_pool,
            local_defs,
        } = self;
        graphs.clear();
        z_order.clear();
        nodes.clear();
        connections.clear();
        subscriptions.clear();
        selected.clear();
        inputs.clear();
        outputs.clear();
        events.clear();
        value_variants_pool.clear();
        local_defs.clear();
    }

    /// Project one graph's nodes, wiring, and view state onto the end of
    /// every pool, returning the [`SceneGraph`] that slices them back out.
    fn project(
        &mut self,
        ui: &mut Ui,
        library: &Library,
        run_state: &RunState,
        projection: GraphProjection<'_>,
        empty: &InternedStr,
    ) -> SceneGraph {
        let GraphProjection {
            target,
            source,
            view,
        } = projection;
        let (graph, interface) = (source.graph(), source.interface());
        // A definition pane is no particular instance of that definition, so
        // a node there has an occurrence per instance and a run raised over
        // the pane has no single one to target — and a preview spawned there
        // would be an entry-only func in a definition body, a compile error.
        let run_available = target == GraphRef::Main;

        // `GraphView::selected` is a `BTreeSet`, so this lands sorted —
        // which is what `GraphScene::is_selected` binary-searches.
        let selected = extend_pool(&mut self.selected, view.selected.iter().copied());
        let z_start = self.z_order.len();
        let nodes_start = self.nodes.len();

        for (key, position) in &view.item_placements {
            let id = *key;
            let Some(node) = graph.find(id, NodeSearch::TopLevel) else {
                continue;
            };
            let node_interface = NodeInterface::resolve(ui, graph, library, node, interface, empty);
            let ports = self.push_node_ports(
                ui,
                library,
                projection,
                id,
                &node_interface,
                run_state.missing_inputs(id),
                empty,
            );
            let boundary = matches!(node.kind, NodeKind::GraphInput | NodeKind::GraphOutput);
            // Resolved before the literal below, which moves `description`
            // out of `node_interface` and so ends the borrow these need.
            let cache_controls = node_interface.cache_controls();
            let can_evict_cache = node_interface.can_evict_cache(boundary);
            // Boundary nodes carry no name in the model; label them by
            // role so the interior header isn't a blank bar.
            let name = match (&node.kind, node.name.is_empty()) {
                (NodeKind::GraphInput, true) => ui.intern("Inputs"),
                (NodeKind::GraphOutput, true) => ui.intern("Outputs"),
                _ => intern_or_empty(ui, empty, &node.name),
            };
            let previous = self.nodes.insert(
                id,
                SceneNode {
                    id,
                    owner: target,
                    pos: *position,
                    name,
                    kind_label: node_interface.kind_label,
                    description: node_interface.description,
                    inputs: ports.inputs,
                    outputs: ports.outputs,
                    events: ports.events,
                    graph: node_interface.graph,
                    // The compiled program is the authority here: a composite
                    // dissolves, so whether it performs sink work is a fact
                    // about its interior, not about its own port arity. With
                    // nothing compiled to fold — before the first run, or a
                    // definition no instance reaches — the interface's own
                    // reading stands.
                    sink: run_state.is_sink(id).unwrap_or(node_interface.sink),
                    disabled: node.disabled,
                    cache: node.cache,
                    cache_controls,
                    can_evict_cache,
                    impure: run_state.is_impure(id).unwrap_or(node_interface.impure),
                    boundary,
                    exec_status: run_state.status(id),
                    ram: run_state.ram(id),
                    missing: node_interface.missing,
                    preview: !node_interface.missing
                        && matches!(node.kind, NodeKind::Func(func_id) if preview::is_preview(func_id)),
                },
            );
            // A repeat would silently overwrite the earlier graph's entry and
            // leave both node spans short — the one way the shared pool could
            // misbehave, and exactly what `Document::validate` rules out.
            debug_assert!(
                previous.is_none(),
                "node ids are document-unique, so one pool spans every graph"
            );
            self.z_order.push(*key);
        }

        let connections = extend_pool(
            &mut self.connections,
            graph.edges().map(|(tgt, src)| SceneConnection { src, tgt }),
        );
        let subscriptions = extend_pool(&mut self.subscriptions, graph.subscriptions());
        let local_defs_start = self.local_defs.len();
        self.local_defs
            .extend(graph.graphs.iter().map(|(id, def)| SceneLocalDef {
                id: *id,
                name: intern_or_empty(ui, empty, &def.interface.name),
                category: intern_or_empty(ui, empty, &def.interface.category),
                origin: def.interface.origin,
            }));
        // `Graph::graphs` is a `HashMap`. Its order reaches the palette, whose
        // own sort is by name and stable, so two same-named defs would swap
        // rows between frames unless the span itself is ordered.
        self.local_defs[local_defs_start..].sort_unstable_by_key(|def| def.id);
        SceneGraph {
            target,
            viewport: view.viewport,
            nodes: span_since(nodes_start, self.nodes.len()),
            z_order: span_since(z_start, self.z_order.len()),
            connections,
            subscriptions,
            selected,
            local_defs: span_since(local_defs_start, self.local_defs.len()),
            run_available,
        }
    }

    /// Append one node's ports to the three per-port pools (plus the picker
    /// options under the inputs) and hand back the spans that slice them
    /// out again. `missing_inputs` is the last run's verdict for this node — the
    /// port indices it could not feed.
    // One over the lint's threshold, and every argument is a distinct source the ports
    // are built from — bundling any two would only hide which one a field came from.
    #[allow(clippy::too_many_arguments)]
    fn push_node_ports(
        &mut self,
        ui: &mut Ui,
        library: &Library,
        projection: GraphProjection<'_>,
        id: NodeId,
        node_interface: &NodeInterface<'_>,
        missing_inputs: &[usize],
        empty: &InternedStr,
    ) -> NodePortSpans {
        let graph = projection.source.graph();
        // One `SceneInput` per input port. Each input's value_variants are
        // flattened into the shared pool, the input recording its span
        // (empty for the common no-options case) — so this one can't go
        // through `extend_pool`, which would borrow two pools at once.
        let inputs_start = self.inputs.len();
        for (port_idx, input) in node_interface.inputs.iter().enumerate() {
            let value_variants = extend_pool(
                &mut self.value_variants_pool,
                input.value_variants.iter().cloned(),
            );
            let port = InputPort::new(id, port_idx);
            self.inputs.push(SceneInput {
                name: ui.intern(&input.name),
                description: intern_or_empty(
                    ui,
                    empty,
                    input.description.as_deref().unwrap_or_default(),
                ),
                ty: input.data_type.clone(),
                binding: InputBindingView::from(graph.bindings.get(&port)),
                default: default_static_value(library, input),
                required: input.required,
                missing: missing_inputs.contains(&port_idx),
                const_only: input.const_only,
                value_variants,
            });
        }
        let inputs = span_since(inputs_start, self.inputs.len());
        let outputs = extend_pool(
            &mut self.outputs,
            node_interface
                .outputs
                .iter()
                .enumerate()
                .map(|(i, o)| SceneOutput {
                    name: ui.intern(&o.name),
                    description: intern_or_empty(
                        ui,
                        empty,
                        o.description.as_deref().unwrap_or_default(),
                    ),
                    // A wildcard output (passthrough / reroute) reports the type
                    // resolved through the input it mirrors; a fixed output uses
                    // its declared type.
                    ty: match &o.ty {
                        OutputType::Wildcard { .. } => {
                            graph.resolve_output_type(library, OutputPort::new(id, i))
                        }
                        OutputType::Fixed(dt) => dt.clone(),
                    },
                }),
        );
        let events = match node_interface.events {
            None => Span::default(),
            Some(events) => extend_pool(
                &mut self.events,
                events.names().map(|name| SceneEvent {
                    name: ui.intern(name),
                }),
            ),
        };
        NodePortSpans {
            inputs,
            outputs,
            events,
        }
    }

    /// Every projected graph, in dock-pane order — the per-pane loop the
    /// canvas prepass and the editor's target bookkeeping both walk.
    pub(crate) fn graphs(&self) -> impl Iterator<Item = GraphScene<'_>> {
        self.graphs
            .values()
            .map(move |graph| GraphScene { scene: self, graph })
    }

    /// The named pane's projection, or `None` when that graph isn't on
    /// screen this frame.
    pub(crate) fn graph(&self, target: GraphRef) -> Option<GraphScene<'_>> {
        self.graphs
            .get(&target)
            .map(|graph| GraphScene { scene: self, graph })
    }

    /// The pane `node_id` lives in — how a whole-scene scan turns a node hit
    /// back into the graph whose edit target the intent belongs to.
    pub(crate) fn owner(&self, node_id: NodeId) -> Option<GraphScene<'_>> {
        self.graph(self.nodes.get(&node_id)?.owner)
    }
}

impl<'a> GraphScene<'a> {
    /// The editing target every intent raised over this pane commits
    /// against.
    pub(crate) fn target(self) -> GraphRef {
        self.graph.target
    }

    pub(crate) fn viewport(self) -> Viewport {
        self.graph.viewport
    }

    /// Whether a run raised over this pane resolves to one occurrence — see
    /// [`SceneGraph::run_available`]. Gates the affordances that need a
    /// single target: the header play chip, the context menu's "Run to this
    /// node", and the port menu's "Add preview".
    pub(crate) fn run_available(self) -> bool {
        self.graph.run_available
    }

    /// Whether `node` can seed a "run to this node" — drives the header play
    /// chip and the context-menu item. Both halves
    /// have to hold: the pane resolves to one occurrence, and the node itself
    /// covers compiled work. Disabled nodes remain valid because a targeted
    /// run overrides that flag temporarily.
    ///
    /// A method on the pane, not the node: "is a run targetable here" is a
    /// fact about the pane, and every caller already has one in hand.
    pub(crate) fn runnable(self, node: &SceneNode) -> bool {
        self.run_available() && node.executable_kind()
    }

    /// This graph's nodes, in relative paint order.
    pub(crate) fn nodes(self) -> impl Iterator<Item = &'a SceneNode> {
        self.scene
            .nodes
            .get_range(self.graph.nodes.range())
            .into_iter()
            .flat_map(|slice| slice.values())
    }

    /// A node of *this* graph by id. Filtered by owner, so a pane never
    /// resolves an id belonging to another open pane.
    pub(crate) fn node(self, node_id: NodeId) -> Option<&'a SceneNode> {
        self.scene
            .nodes
            .get(&node_id)
            .filter(|n| n.owner == self.graph.target)
    }

    pub(crate) fn contains(self, node_id: NodeId) -> bool {
        self.node(node_id).is_some()
    }

    /// This graph's paint stack: node bodies
    /// interleaved, later entries in front.
    pub(crate) fn z_order(self) -> &'a [NodeId] {
        slice_pool(&self.scene.z_order, self.graph.z_order)
    }

    pub(crate) fn connections(self) -> &'a [SceneConnection] {
        slice_pool(&self.scene.connections, self.graph.connections)
    }

    pub(crate) fn subscriptions(self) -> &'a [Subscription] {
        slice_pool(&self.scene.subscriptions, self.graph.subscriptions)
    }

    /// This graph's own local definitions, ordered by id.
    pub(crate) fn local_defs(self) -> &'a [SceneLocalDef] {
        slice_pool(&self.scene.local_defs, self.graph.local_defs)
    }

    /// This graph's committed selection, sorted.
    pub(crate) fn selected(self) -> &'a [NodeId] {
        slice_pool(&self.scene.selected, self.graph.selected)
    }

    /// Whether `key` is in this graph's committed selection — a binary
    /// search, since the span is kept sorted.
    pub(crate) fn is_selected(self, key: NodeId) -> bool {
        selection_holds(self.selected(), key)
    }

    /// The committed selection as the owned set an `Intent::SetSelection`
    /// carries.
    pub(crate) fn selection(self) -> BTreeSet<NodeId> {
        self.selected().iter().copied().collect()
    }

    /// A node's input ports, sliced by its `inputs` span. The per-port
    /// pools are shared across every pane and addressed only by span, so
    /// these four take no account of which graph `self` is — they sit here
    /// rather than on [`Scene`] because a node is always reached through
    /// the pane that owns it.
    pub(crate) fn inputs(self, span: Span) -> &'a [SceneInput] {
        slice_pool(&self.scene.inputs, span)
    }

    /// A node's output ports, sliced by its `outputs` span.
    pub(crate) fn outputs(self, span: Span) -> &'a [SceneOutput] {
        slice_pool(&self.scene.outputs, span)
    }

    /// A node's event (emitter) ports, sliced by its `events` span.
    pub(crate) fn events(self, span: Span) -> &'a [SceneEvent] {
        slice_pool(&self.scene.events, span)
    }

    /// One input's picker options, resolved from its
    /// [`SceneInput::value_variants`] span into the shared pool.
    pub(crate) fn value_variants(self, span: Span) -> &'a [ValueVariant] {
        slice_pool(&self.scene.value_variants_pool, span)
    }
}

/// Where one node's ports landed in the three per-port pools — the result
/// of [`Scene::push_node_ports`], copied straight onto its [`SceneNode`].
#[derive(Debug)]
struct NodePortSpans {
    inputs: Span,
    outputs: Span,
    events: Span,
}

/// Whether a **sorted** selection slice holds `key`.
///
/// Every selection slice in the GUI is sorted — a graph's committed span
/// (`GraphView::selected` is a `BTreeSet`, so
/// [`Scene::project`] extends in order) and `SelectionUI`'s rubber-band
/// preview (collected from a `BTreeSet` too). That invariant is what makes
/// membership a binary search instead of a set lookup, so it's asserted in
/// one place here rather than assumed independently at each of the three
/// call sites.
pub(crate) fn selection_holds(selected: &[NodeId], key: NodeId) -> bool {
    debug_assert!(
        selected.is_sorted(),
        "selection slices are kept sorted so membership can binary-search"
    );
    selected.binary_search(&key).is_ok()
}

fn slice_pool<T>(pool: &[T], span: Span) -> &[T] {
    &pool[span.range()]
}

/// The [`Span`] covering `start..end` of a pool.
fn span_since(start: usize, end: usize) -> Span {
    Span::new(start as u32, (end - start) as u32)
}

/// View of a node's interface during a single rebuild: the input ports
/// (whole `FuncInput`, for names + defaults) and the output port names.
/// Inputs are usually borrowed from a func/graph; the `GraphOutput`
/// boundary node synthesizes them from the graph's `FuncOutput`s, hence
/// `Cow`.
#[derive(Debug)]
struct NodeInterface<'a> {
    kind_label: InternedStr,
    /// The func's description (empty for graphs/boundaries/missing stubs).
    description: InternedStr,
    inputs: Cow<'a, [FuncInput]>,
    outputs: Cow<'a, [FuncOutput]>,
    /// Event (emitter) ports, in declaration order. `FuncEvent` and
    /// `GraphEvent` differ in type, so they can't share a slice — but
    /// [`NodeEvents`] is a `Copy` borrow of either, which is all the UI
    /// needs to read the names off at pool-fill time. Borrowed rather than
    /// eagerly interned into a `Vec`, which cost one heap allocation per
    /// node per frame for a list that was copied into `Scene::events` and
    /// dropped immediately. `None` for boundary and missing-stub nodes,
    /// which expose no events at all.
    events: Option<NodeEvents<'a>>,
    graph: Option<GraphLink>,
    sink: bool,
    /// Node manages its own caching (or has no output to cache), so the editor's
    /// cache chips are hidden — see [`SceneNode::cache_controls`].
    uncacheable: bool,
    /// The func is `Impure`, so it has no content digest and no cache mode is
    /// honored — the editor hides its cache chips (see
    /// [`SceneNode::impure`]). Only set from a func spec; `false` for composites
    /// (aggregate purity isn't known here) and boundary/stub nodes.
    impure: bool,
    /// Nothing in the library answered to this node's func/graph, so what's
    /// here is the portless error stub — see [`SceneNode::missing`].
    missing: bool,
}

impl<'a> NodeInterface<'a> {
    /// The interface `node` projects with: read off its func, its referenced
    /// definition, or — for a boundary node — the enclosing definition's own
    /// interface, which only a `GraphDef` carries. Scenarium resolves the
    /// first three into one `NodePorts`; only the boundary pair, whose ports
    /// the editor synthesizes, is spelled out here.
    ///
    /// A func or graph node whose target is absent has no interface to
    /// project. Dropping it would make it invisible — and so impossible to
    /// select and delete — silently corrupting the document, so it comes
    /// back as a portless `missing` stub instead.
    ///
    /// Reads nothing off `Scene`: an interface is a pure function of the
    /// node and the library, and only the pooling that follows it needs the
    /// projection's mutable state.
    ///
    /// `graph`, `library`, and `node` share `'a`: a resolved `NodePorts`
    /// borrows its port slices from all three, and they land in the `Cow`s
    /// below unchanged.
    fn resolve(
        ui: &mut Ui,
        graph: &'a Graph,
        library: &'a Library,
        node: &'a Node,
        interface: Option<&GraphInterface>,
        empty: &InternedStr,
    ) -> Self {
        let resolved = match &node.kind {
            NodeKind::Func(_) | NodeKind::Graph(_) | NodeKind::Special(_) => {
                graph.node_ports(node, library).map(|ports| NodeInterface {
                    kind_label: ui.intern(ports.name),
                    description: intern_or_empty(ui, empty, ports.description.unwrap_or_default()),
                    inputs: Cow::Borrowed(ports.inputs),
                    outputs: Cow::Borrowed(ports.outputs),
                    events: Some(ports.events),
                    graph: match node.kind {
                        NodeKind::Graph(link) => Some(link),
                        _ => None,
                    },
                    // A composite has no declaration of its own to read, so
                    // "exposes no outputs" stands in until a compiled program
                    // can answer for its interior (`Scene::project` folds
                    // `CompiledGraph::is_sink` over this).
                    sink: ports
                        .func
                        .map_or_else(|| ports.outputs.is_empty(), |func| func.sink),
                    uncacheable: ports.func.is_some_and(|func| func.uncacheable),
                    // A composite has no declaration to read, and its interior's
                    // aggregate purity isn't knowable here — `Scene::project`
                    // folds `CompiledGraph::is_impure` over this once a program
                    // exists.
                    impure: ports
                        .func
                        .is_some_and(|func| func.behavior == FuncBehavior::Impure),
                    missing: false,
                })
            }
            // Inbound boundary: no inputs; one output per graph input,
            // plus a trailing placeholder output. Wiring the
            // placeholder emits `AddBoundaryPort` + `SetInput` as one
            // batch (see `connection_ui::commit_connection`), so a
            // fresh placeholder appears next frame.
            NodeKind::GraphInput => interface.map(|interface| {
                let mut outputs: Vec<FuncOutput> =
                    interface.inputs.iter().map(boundary_output).collect();
                outputs.push(placeholder_output());
                NodeInterface::synthesized(
                    ui.intern("Input"),
                    empty.clone(),
                    Cow::Borrowed(&[]),
                    Cow::Owned(outputs),
                )
            }),
            // Outbound boundary: one input per graph output (synthesized
            // as `FuncInput`s for names + zero defaults), plus a
            // trailing placeholder input. Symmetric to the inbound
            // case — wiring the placeholder grows the definition's outputs.
            NodeKind::GraphOutput => interface.map(|interface| {
                let mut inputs: Vec<FuncInput> =
                    interface.outputs.iter().map(boundary_input).collect();
                inputs.push(placeholder_input());
                NodeInterface::synthesized(
                    ui.intern("Output"),
                    empty.clone(),
                    Cow::Owned(inputs),
                    Cow::Borrowed(&[]),
                )
            }),
        };
        // A `match` rather than `unwrap_or_else`: the stub borrows nothing
        // from `node` or `library`, but a closure returning `Self<'a>` makes
        // the compiler tie their lifetimes to `'a` anyway.
        match resolved {
            Some(resolved) => resolved,
            None => {
                let kind_label = match node.kind {
                    NodeKind::Func(_) => "missing func",
                    NodeKind::Graph(_) => "missing graph",
                    // A special node's spec always resolves. A boundary
                    // node only exists in a definition body, and
                    // `SceneSource::Def` carries that body's interface
                    // inseparably — so neither reaches this branch.
                    NodeKind::Special(_) | NodeKind::GraphInput | NodeKind::GraphOutput => {
                        unreachable!("special and boundary interfaces always resolve")
                    }
                };
                NodeInterface {
                    missing: true,
                    ..NodeInterface::synthesized(
                        ui.intern(kind_label),
                        empty.clone(),
                        Cow::Borrowed(&[]),
                        Cow::Borrowed(&[]),
                    )
                }
            }
        }
    }

    /// An interface the editor synthesizes rather than reads off a func or a
    /// definition: the two boundary nodes, and the stub standing in for a
    /// node whose target is gone. None of them emits events, instantiates a
    /// graph, sinks, or has anything cacheable — only the label, the ports,
    /// and (for the stub) `missing` differ between the three.
    fn synthesized(
        kind_label: InternedStr,
        description: InternedStr,
        inputs: Cow<'a, [FuncInput]>,
        outputs: Cow<'a, [FuncOutput]>,
    ) -> Self {
        Self {
            kind_label,
            description,
            inputs,
            outputs,
            events: None,
            graph: None,
            sink: false,
            uncacheable: true,
            impure: false,
            missing: false,
        }
    }

    /// Whether the header offers the RAM/disk storage chips — see
    /// [`SceneNode::cache_controls`]. A composite's storage is its interior's
    /// business, an impure func has no content digest to key a cache on, and
    /// a func that declares itself uncacheable or exposes no outputs has
    /// nothing to store.
    fn cache_controls(&self) -> bool {
        self.graph.is_none() && !self.uncacheable && !self.outputs.is_empty() && !self.impure
    }

    /// Whether the header offers runtime cache eviction — see
    /// [`SceneNode::can_evict_cache`]. A graph instance always can, evicting
    /// its flattened interior; anything else needs a reproducible output,
    /// which rules out impure funcs, portless nodes, and the boundary pair.
    fn can_evict_cache(&self, boundary: bool) -> bool {
        self.graph.is_some() || (!self.outputs.is_empty() && !self.impure && !boundary)
    }
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
                .enum_variants(*id)
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

/// Synthesize a `FuncInput` for a `GraphOutput`'s input port from the
/// graph output it mirrors — name + type carry over; it's not user-set, so
/// it has no declared default and no value options.
fn boundary_input(output: &FuncOutput) -> FuncInput {
    FuncInput::optional(output.name.clone(), output.ty.declared())
}

/// Synthesize a `FuncOutput` for a `GraphInput`'s output port from the
/// graph input it mirrors — name + type carry over.
fn boundary_output(input: &FuncInput) -> FuncOutput {
    FuncOutput::new(input.name.clone(), input.data_type.clone())
}

/// The trailing "connect here to add a port" placeholder output on the
/// `GraphInput` boundary node: untyped until something connects.
fn placeholder_output() -> FuncOutput {
    FuncOutput::new(PLACEHOLDER_PORT, DataType::default())
}

/// Label for the trailing "connect here to add a port" placeholder on a
/// boundary node.
const PLACEHOLDER_PORT: &str = "+";

/// The placeholder input port for a `GraphOutput`: unbound, untyped
/// until something connects (at which point the commit's
/// `AddBoundaryPort` materializes a real definition output typed from
/// the wired producer).
fn placeholder_input() -> FuncInput {
    FuncInput::optional(PLACEHOLDER_PORT, DataType::default())
}

fn extend_pool<T>(pool: &mut Vec<T>, items: impl IntoIterator<Item = T>) -> Span {
    let start = pool.len();
    pool.extend(items);
    span_since(start, pool.len())
}

/// Intern `text`, reusing the pre-made `empty` handle when it has none.
///
/// `Ui::intern` takes the arena's `RefCell` and clones its `Rc` even for
/// `""`, and empty is the *common* case here: most ports declare no
/// description and most nodes carry no authored name. Cloning one handle
/// instead is a span copy and a refcount bump.
fn intern_or_empty(ui: &mut Ui, empty: &InternedStr, text: &str) -> InternedStr {
    if text.is_empty() {
        empty.clone()
    } else {
        ui.intern(text)
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;

    /// Minimal node for viewport/bounds math tests: identity + position
    /// only, every render field defaulted.
    pub(crate) fn scene_node_stub(ui: &mut Ui, id: NodeId, pos: Vec2) -> SceneNode {
        SceneNode {
            id,
            owner: GraphRef::Main,
            pos,
            name: ui.intern(""),
            kind_label: ui.intern(""),
            description: ui.intern(""),
            inputs: Span::default(),
            outputs: Span::default(),
            events: Span::default(),
            graph: None,
            sink: false,
            disabled: false,
            cache: CacheMode::None,
            cache_controls: false,
            can_evict_cache: false,
            impure: false,
            boundary: false,
            exec_status: ExecStatus::None,
            ram: RamUsage::default(),
            missing: false,
            preview: false,
        }
    }

    impl Scene {
        /// A one-pane scene at `GraphRef::Main` holding `nodes` — the
        /// fixture the gesture tests need, without a `Document` or a
        /// `Library` behind it. Extend it with [`Self::with_selection`] /
        /// [`Self::with_outputs`].
        pub(crate) fn with_nodes(nodes: impl IntoIterator<Item = SceneNode>) -> Self {
            let mut scene = Scene::default();
            for node in nodes {
                scene.z_order.push(node.id);
                scene.nodes.insert(node.id, node);
            }
            scene.reseal();
            scene
        }

        /// Give the sole graph a committed selection.
        pub(crate) fn with_selection(mut self, selected: impl IntoIterator<Item = NodeId>) -> Self {
            self.selected.extend(selected);
            self.selected.sort();
            self.selected.dedup();
            self.reseal();
            self
        }

        /// Mark the sole graph as a definition pane: no single occurrence for
        /// a run to target, so [`GraphScene::runnable`] answers `false`
        /// whatever the node is. Order-independent like the other builders —
        /// [`Self::reseal`] carries the flag forward.
        pub(crate) fn without_run_target(mut self) -> Self {
            self.reseal();
            let graph = self.graphs.get_mut(&GraphRef::Main).expect("resealed");
            graph.run_available = false;
            self
        }

        /// Re-derive the sole graph's spans over whatever the pools now
        /// hold, so the builder methods can be chained in any order.
        fn reseal(&mut self) {
            // A resealed graph keeps whatever run availability it was given;
            // a fresh one stands in for the root pane, where a run resolves.
            let run_available = self
                .graphs
                .get(&GraphRef::Main)
                .is_none_or(|graph| graph.run_available);
            self.graphs.insert(
                GraphRef::Main,
                SceneGraph {
                    target: GraphRef::Main,
                    viewport: Viewport::default(),
                    nodes: Span::new(0, self.nodes.len() as u32),
                    z_order: Span::new(0, self.z_order.len() as u32),
                    connections: Span::default(),
                    subscriptions: Span::default(),
                    selected: Span::new(0, self.selected.len() as u32),
                    local_defs: Span::new(0, self.local_defs.len() as u32),
                    run_available,
                },
            );
        }

        /// The sole graph of a single-pane test scene.
        pub(crate) fn only_graph(&self) -> GraphScene<'_> {
            self.graph(GraphRef::Main).expect("test scene has Main")
        }
    }
}

#[cfg(test)]
mod tests;
