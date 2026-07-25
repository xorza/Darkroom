use std::borrow::Cow;
use std::collections::BTreeSet;

use aperture::{InternedStr, Ui};
use common::Span;
use glam::Vec2;
use indexmap::IndexMap;
use scenarium::GraphLink;
use scenarium::Library;
use scenarium::{
    Binding, CacheMode, Graph, InputPort, NodeId, NodeKind, NodeSearch, OutputPort, Subscription,
};
use scenarium::{DataType, GraphDef, GraphInterface, NodeEvents, RamUsage, StaticValue};
use scenarium::{FuncBehavior, FuncInput, FuncOutput, OutputType, ValueVariant};

use crate::core::document::{GraphView, ItemRef, PortKind, PortRef, Viewport};
use crate::gui::EventRef;
use crate::gui::run_state::{ExecStatus, RunState};

#[derive(Debug, Default)]
pub(crate) struct Scene {
    /// The shared paint stack, mirrored from `GraphView::item_placements` order:
    /// node bodies and pinned-output previews interleaved, later entries
    /// drawn in front. The canvas draw pass iterates this and dispatches on
    /// the key kind; everything else looks items up through `nodes`.
    pub(crate) z_order: Vec<ItemRef>,
    /// Keyed node projections in relative paint order. Interaction scans use
    /// this order to resolve overlapping node and port hits.
    pub(crate) nodes: IndexMap<NodeId, SceneNode>,
    pub(crate) connections: Vec<SceneConnection>,
    /// Event-subscription edges (emitter event → subscriber node), mirrored
    /// from the active graph each rebuild. Drawn as event wires; the editor
    /// adds/removes them via the subscription drag gesture. The model's
    /// `Subscription` is a plain `Copy` id-bundle with no render-only fields,
    /// so it's mirrored verbatim (unlike `SceneConnection`, which flattens).
    pub(crate) subscriptions: Vec<Subscription>,
    /// One flat pool of [`SceneInput`] across every node, sliced by the single
    /// `SceneNode::inputs` span. A struct-per-port (not parallel columns) so the
    /// per-port fields can't desync. Keeps per-node allocations to zero in
    /// steady state (the `Vec` retains capacity across the per-frame `clear` +
    /// re-`extend`).
    pub(crate) inputs: Vec<SceneInput>,
    /// One flat pool of [`SceneOutput`] across every node, sliced by the single
    /// `SceneNode::outputs` span.
    pub(crate) outputs: Vec<SceneOutput>,
    /// One flat pool of [`SceneEvent`] across every node, sliced by the single
    /// `SceneNode::events` span. Events are emitter ports (always outgoing), so
    /// the UI lists them under the output ports.
    pub(crate) events: Vec<SceneEvent>,
    /// One flat pool of every input's picker options across all nodes, sliced
    /// per input by [`SceneInput::value_variants`].
    pub(crate) value_variants_pool: Vec<ValueVariant>,
    /// Live viewport, mirrored from the active `GraphView` each `rebuild`.
    /// Read by the canvas transform and the pointer↔world mapping; the
    /// pan/zoom gesture writes it back here, and `App` copies it onto the
    /// document so it persists (single owner = `Document`).
    pub(crate) viewport: Viewport,
    /// Currently-selected nodes and pinned-output previews, the committed
    /// set mirrored from `Document` each rebuild so `node_ui`/`pin_ui` can
    /// pick a different paint without taking a `&Document`. Read-only, like
    /// the rest of `Scene`: the in-progress rubber-band preview lives on
    /// `SelectionUI` (read back via `SelectionUI::preview`) and the canvas
    /// unions the two when drawing, so the gesture never writes into this
    /// projection.
    pub(crate) selected: BTreeSet<ItemRef>,
}

/// Per-frame snapshot of an input port's [`Binding`] for the UI tree.
/// Variant-only for `Bind`; the address details live on `Scene::connections`.
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
    /// A required input with no binding is a missing input — its port renders
    /// highlighted.
    pub(crate) required: bool,
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
    /// The pinned preview widget's top-left corner in absolute
    /// canvas-world coordinates — `Some` iff this output is pinned
    /// (kept computed and read even with no in-graph consumer — see
    /// [`scenarium::Graph::is_output_pinned`]). Mirrors the port's
    /// `GraphView::item_placements` entry, which exists exactly while pinned,
    /// so pinned-ness and position can't desync.
    pub(crate) pin_position: Option<Vec2>,
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
    pub(crate) pos: Vec2,
    pub(crate) name: InternedStr,
    /// Human-readable type identity: the func name, the graph
    /// name, or the boundary role (`Input`/`Output`). Shown by the
    /// inspection panel.
    pub(crate) kind_label: InternedStr,
    /// The func's [`Func::description`] (empty for graph/boundary nodes).
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
    /// The node's func is `Impure`. An impure node has no content digest, so no
    /// cache mode is ever honored (folded into the cache controls); the header also
    /// paints the `~` marker off this flag to say *why* it has no cache controls.
    /// `false` for composites and boundary nodes.
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
    /// RAM this node's cached output currently holds (system vs GPU), mirrored
    /// from `run_state`. Non-zero only for nodes that retain a value; drives the
    /// node body's memory readout, hidden when zero.
    pub(crate) ram: RamUsage,
    /// The node's func/graph is absent from the library (e.g. a
    /// document saved against an older library), so its interface can't be
    /// resolved. Rendered as a portless error stub the user can still
    /// select and delete — never silently dropped.
    pub(crate) missing: bool,
    /// Whether this editor target has an exact identity in the root compiled
    /// program. Local definition tabs lack their enclosing instance path.
    pub(crate) run_available: bool,
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

    fn executable_kind(&self) -> bool {
        !self.boundary && !self.missing && self.graph.is_none()
    }

    /// Whether this node can seed a "run to this node" — drives the header
    /// play chip and the context-menu item. Disabled func nodes remain valid
    /// because a targeted run overrides that flag temporarily. An instance/
    /// boundary node has no execution identity, and a `missing` stub resolves to
    /// nothing.
    pub(crate) fn runnable(&self) -> bool {
        self.run_available && self.executable_kind()
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

/// What a rebuild projects. An enum rather than a graph + optional
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
    /// Names are arena-backed handles authored through this record pass's
    /// `Ui`. Rebuilding keeps the projection synchronized with the graph and
    /// lets the previous pass's text arena be recycled. `App::record` enforces
    /// this before widgets consume the scene.
    ///
    pub(crate) fn rebuild(
        &mut self,
        ui: &mut Ui,
        source: SceneSource<'_>,
        view: &GraphView,
        library: &Library,
        run_state: &RunState,
        run_available: bool,
    ) {
        let (graph, interface) = (source.graph(), source.interface());
        self.selected = view.selected.clone();
        // Mirror the persisted viewport. The gesture overwrites this
        // later this frame and `App` copies it back onto the doc, so
        // a fresh value (e.g. just-loaded document) shows up here while
        // an active pan/zoom isn't clobbered (it re-persists each frame).
        self.viewport = view.viewport;
        self.z_order.clear();
        self.nodes.clear();
        self.connections.clear();
        self.subscriptions.clear();
        self.inputs.clear();
        self.outputs.clear();
        self.events.clear();
        self.value_variants_pool.clear();

        // One handle for the empty string, cloned wherever a port declares no
        // description or a node carries no authored name — see
        // `intern_or_empty`.
        let empty = ui.intern("");

        for (key, position) in &view.item_placements {
            let id = match *key {
                ItemRef::Node(id) => id,
                ItemRef::Pin(_) => {
                    // A pin's card draws from its owner's `SceneOutput`;
                    // only its slot in the shared paint order lives here.
                    self.z_order.push(*key);
                    continue;
                }
            };
            let Some(node) = graph.find(id, NodeSearch::TopLevel) else {
                continue;
            };
            // A node's interface comes from its func, its referenced
            // definition, or — for a boundary node — the enclosing
            // definition's own interface, which only a `GraphDef` carries.
            // Scenarium resolves the first three into one `NodePorts`; only
            // the boundary pair, whose ports the editor synthesizes, is
            // spelled out here.
            let interface = match &node.kind {
                NodeKind::Func(_) | NodeKind::Graph(_) | NodeKind::Special(_) => {
                    graph.node_ports(node, library).map(|ports| NodeInterface {
                        kind_label: ui.intern(ports.name),
                        description: intern_or_empty(
                            ui,
                            &empty,
                            ports.description.unwrap_or_default(),
                        ),
                        inputs: Cow::Borrowed(ports.inputs),
                        outputs: Cow::Borrowed(ports.outputs),
                        events: Some(ports.events),
                        graph: match node.kind {
                            NodeKind::Graph(link) => Some(link),
                            _ => None,
                        },
                        // A composite's sink-ness is derived at flatten time,
                        // not stored separately; treat "no exposed outputs" as
                        // the visible sink signal.
                        sink: ports
                            .func
                            .map_or_else(|| ports.outputs.is_empty(), |func| func.sink),
                        uncacheable: ports.func.is_some_and(|func| func.uncacheable),
                        // Aggregate purity of a composite isn't known here, so the
                        // cache chips stay available for it (unlike a func).
                        impure: ports
                            .func
                            .is_some_and(|func| func.behavior == FuncBehavior::Impure),
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
            // A func or graph node whose target is absent
            // has no interface to project. Dropping it would make it
            // invisible — and so impossible to select and delete — silently
            // corrupting the document. Render it as a portless error stub
            // instead.
            let missing = interface.is_none();
            let interface = match interface {
                Some(interface) => interface,
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
                    NodeInterface::synthesized(
                        ui.intern(kind_label),
                        empty.clone(),
                        Cow::Borrowed(&[]),
                        Cow::Borrowed(&[]),
                    )
                }
            };
            // One `SceneInput` per input port, sliced by the node's `inputs`
            // span. Each input's value_variants are flattened into one pool,
            // the input recording its span (empty for the common no-options case).
            let inputs_start = self.inputs.len();
            for (port_idx, input) in interface.inputs.iter().enumerate() {
                let value_variants = extend_pool(
                    &mut self.value_variants_pool,
                    input.value_variants.iter().cloned(),
                );
                let port = InputPort::new(id, port_idx);
                self.inputs.push(SceneInput {
                    name: ui.intern(&input.name),
                    description: intern_or_empty(
                        ui,
                        &empty,
                        input.description.as_deref().unwrap_or_default(),
                    ),
                    ty: input.data_type.clone(),
                    binding: InputBindingView::from(graph.bindings.get(&port)),
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
                        name: ui.intern(&o.name),
                        description: intern_or_empty(
                            ui,
                            &empty,
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
                        pin_position: view
                            .item_placements
                            .get(&ItemRef::Pin(OutputPort::new(id, i)))
                            .copied(),
                    }),
            );
            let events = match interface.events {
                None => Span::default(),
                Some(events) => extend_pool(
                    &mut self.events,
                    events.names().map(|name| SceneEvent {
                        name: ui.intern(name),
                    }),
                ),
            };
            let boundary = matches!(node.kind, NodeKind::GraphInput | NodeKind::GraphOutput);
            // Resolved before the literal below, which moves `description`
            // out of `interface` and so ends the borrow these need.
            let cache_controls = interface.cache_controls();
            let can_evict_cache = interface.can_evict_cache(boundary);
            // Boundary nodes carry no name in the model; label them by
            // role so the interior header isn't a blank bar.
            let name = match (&node.kind, node.name.is_empty()) {
                (NodeKind::GraphInput, true) => ui.intern("Inputs"),
                (NodeKind::GraphOutput, true) => ui.intern("Outputs"),
                _ => intern_or_empty(ui, &empty, &node.name),
            };
            self.nodes.insert(
                id,
                SceneNode {
                    id,
                    pos: *position,
                    name,
                    kind_label: interface.kind_label,
                    description: interface.description,
                    inputs,
                    outputs,
                    events,
                    graph: interface.graph,
                    sink: interface.sink,
                    disabled: node.disabled,
                    cache: node.cache,
                    cache_controls,
                    can_evict_cache,
                    impure: interface.impure,
                    boundary,
                    exec_status: run_state.status(id),
                    ram: run_state.ram(id),
                    missing,
                    run_available,
                },
            );
            self.z_order.push(*key);
        }

        for (tgt, src) in graph.edges() {
            self.connections.push(SceneConnection { src, tgt });
        }

        self.subscriptions.extend(graph.subscriptions());
    }

    /// A node's input ports, sliced by its `inputs` span.
    pub(crate) fn inputs(&self, span: Span) -> &[SceneInput] {
        slice_pool(&self.inputs, span)
    }

    /// A node's output ports, sliced by its `outputs` span.
    pub(crate) fn outputs(&self, span: Span) -> &[SceneOutput] {
        slice_pool(&self.outputs, span)
    }

    /// Every pinned output in the scene — the one iteration the pin scans
    /// (drag/click polls, wire draw, rubber-band sweep) share.
    pub(crate) fn pinned_outputs(&self) -> impl Iterator<Item = PinnedOutput<'_>> {
        self.nodes.values().flat_map(|n| {
            self.outputs(n.outputs)
                .iter()
                .enumerate()
                .filter_map(move |(i, output)| {
                    output.pin_position.map(|pos| PinnedOutput {
                        port: OutputPort::new(n.id, i),
                        pos,
                        output,
                    })
                })
        })
    }

    /// A node's event (emitter) ports, sliced by its `events` span.
    pub(crate) fn events(&self, span: Span) -> &[SceneEvent] {
        slice_pool(&self.events, span)
    }

    /// One input's picker options, resolved from its [`SceneInput::value_variants`]
    /// span into the shared pool.
    pub(crate) fn value_variants(&self, span: Span) -> &[ValueVariant] {
        slice_pool(&self.value_variants_pool, span)
    }
}

/// One pinned output surfaced by [`Scene::pinned_outputs`]: its port, its
/// preview widget's top-left corner (the unwrapped
/// [`SceneOutput::pin_position`]), and the projected output.
#[derive(Debug)]
pub(crate) struct PinnedOutput<'a> {
    pub(crate) port: OutputPort,
    pub(crate) pos: Vec2,
    pub(crate) output: &'a SceneOutput,
}

fn slice_pool<T>(pool: &[T], span: Span) -> &[T] {
    &pool[span.range()]
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
    /// cache chips are hidden — see [`SceneNode::uncacheable`].
    uncacheable: bool,
    /// The func is `Impure`, so it has no content digest and no cache mode is
    /// honored — the editor hides its cache chips (see
    /// [`SceneNode::impure`]). Only set from a func spec; `false` for composites
    /// (aggregate purity isn't known here) and boundary/stub nodes.
    impure: bool,
}

impl<'a> NodeInterface<'a> {
    /// An interface the editor synthesizes rather than reads off a func or a
    /// definition: the two boundary nodes, and the stub standing in for a
    /// node whose target is gone. None of them emits events, instantiates a
    /// graph, sinks, or has anything cacheable — only the label and the
    /// ports differ between the three.
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
    Span::new(start as u32, (pool.len() - start) as u32)
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
            run_available: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::scene::internals::scene_node_stub;
    use scenarium::DataType;
    use scenarium::testing;
    use scenarium::{Graph, GraphDef};
    use scenarium::{GraphId, InputPort, Node, OutputPort};

    fn finput(name: &str, ty: DataType) -> FuncInput {
        FuncInput::optional(name, ty)
    }

    #[test]
    fn only_runnable_sinks_expose_the_disable_toggle() {
        let mut ui = Ui::default();
        let mut node = scene_node_stub(&mut ui, NodeId::unique(), Vec2::ZERO);
        assert!(!node.can_disable(), "a non-sink has no disable toggle");

        node.sink = true;
        assert!(node.can_disable(), "a runnable sink can be disabled");

        node.missing = true;
        assert!(
            !node.can_disable(),
            "an unresolved sink cannot be disabled because it cannot be run explicitly"
        );

        node.missing = false;
        node.run_available = false;
        assert!(
            !node.runnable(),
            "a local definition tab has no exact root execution identity"
        );
        assert!(
            node.can_disable(),
            "run availability does not hide the authoring disable toggle"
        );
    }

    #[derive(Debug)]
    struct AdderGraph {
        graph: GraphDef,
        input: NodeId,
        output: NodeId,
    }

    fn adder_graph() -> AdderGraph {
        let in_node = Node::new(NodeKind::GraphInput);
        let out_node = Node::new(NodeKind::GraphOutput);
        let mut graph = GraphDef::new("Adder")
            .category("Graph")
            .inputs([finput("A", DataType::Int), finput("B", DataType::Float)])
            .output(FuncOutput::new("Sum", DataType::Int));
        let input = graph.body.add(in_node);
        let output = graph.body.add(out_node);
        graph
            .body
            .set_input_binding(InputPort::new(output, 0), Binding::bind(input, 0));
        AdderGraph {
            graph,
            input,
            output,
        }
    }

    #[test]
    fn boundary_nodes_mirror_graph_interface() {
        let fixture = adder_graph();
        let view = GraphView::for_graph(&fixture.graph.body);
        let mut scene = Scene::default();
        let mut ui = Ui::default();
        scene.rebuild(
            &mut ui,
            SceneSource::Def(&fixture.graph),
            &view,
            &Library::default(),
            &RunState::default(),
            true,
        );

        assert_eq!(scene.nodes.len(), 2, "both boundary nodes render");
        let expected_node_order = view
            .item_placements
            .keys()
            .filter_map(|item| match item {
                ItemRef::Node(node_id) => Some(*node_id),
                ItemRef::Pin(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            scene.nodes.keys().copied().collect::<Vec<_>>(),
            expected_node_order,
            "node projection follows paint order"
        );
        let input_node = scene.nodes.get(&fixture.input).unwrap();
        let output_node = scene.nodes.get(&fixture.output).unwrap();

        // Boundary nodes are labeled by role.
        assert_eq!(&*input_node.kind_label.borrow_str(), "Input");
        assert_eq!(&*output_node.kind_label.borrow_str(), "Output");

        // GraphInput's outputs mirror the graph inputs (A:Int, B:Float)
        // plus the untyped "+" placeholder — types align with names.
        let in_outs = scene.outputs(input_node.outputs);
        assert_eq!(in_outs.len(), 3, "two graph inputs + placeholder");
        assert!(matches!(in_outs[0].ty, DataType::Int));
        assert!(matches!(in_outs[1].ty, DataType::Float));
        assert!(
            matches!(in_outs[2].ty, DataType::Any),
            "placeholder untyped"
        );

        // GraphOutput's inputs mirror the graph output (Sum:Int) plus a
        // placeholder.
        let out_ins = scene.inputs(output_node.inputs);
        assert_eq!(out_ins.len(), 2, "one graph output + placeholder");
        assert!(matches!(out_ins[0].ty, DataType::Int));

        // GraphInput: 0 inputs; one output per graph *input*, named to
        // match, plus the trailing "+" placeholder.
        assert_eq!(scene.inputs(input_node.inputs).len(), 0);
        let in_out_names: Vec<String> = scene
            .outputs(input_node.outputs)
            .iter()
            .map(|o| o.name.borrow_str().to_owned())
            .collect();
        assert_eq!(in_out_names, ["A", "B", "+"]);
        assert!(input_node.graph.is_none() && !input_node.sink);
        assert!(
            input_node.boundary && output_node.boundary,
            "boundary nodes are flagged so const affordances are suppressed"
        );
        assert!(
            !input_node.runnable() && !output_node.runnable(),
            "boundary nodes offer no run affordance — they have no execution identity"
        );

        // GraphOutput: one input per graph *output* plus the "+"
        // placeholder; 0 outputs.
        assert_eq!(scene.outputs(output_node.outputs).len(), 0);
        let out_in_names: Vec<String> = scene
            .inputs(output_node.inputs)
            .iter()
            .map(|i| i.name.borrow_str().to_owned())
            .collect();
        assert_eq!(out_in_names, ["Sum", "+"]);

        // The interior wire shows up as a connection between the boundaries.
        assert_eq!(scene.connections.len(), 1);
        let c = &scene.connections[0];
        assert_eq!(c.src, OutputPort::new(fixture.input, 0));
        assert_eq!(c.tgt, InputPort::new(fixture.output, 0));
    }

    #[test]
    fn missing_func_and_graph_render_as_deletable_stubs() {
        use scenarium::GraphLink;
        use scenarium::math_library;

        // A resolvable func, plus two unresolvable nodes (e.g. a document
        // saved against an older library): a func id and a shared graph id.
        let library = math_library();
        let mut graph = Graph::default();
        let mut known: Node = library.by_name("Add").unwrap().into();
        known.disabled = true;
        let mut ghost_func = Node::new(NodeKind::Func(
            "7a0265e1-9631-45bd-8ecd-1e923b67a58c".into(),
        ));
        ghost_func.name = "astro_to_image".into();
        let mut ghost_graph = Node::new(NodeKind::Graph(GraphLink::Shared(
            "00000000-0000-0000-0000-0000000000ff".into(),
        )));
        ghost_graph.name = "removed_graph".into();
        let known_id = graph.add(known);
        let ghost_func_id = graph.add(ghost_func);
        let ghost_graph_id = graph.add(ghost_graph);

        let view = GraphView::for_graph(&graph);
        let mut scene = Scene::default();
        let mut ui = Ui::default();
        scene.rebuild(
            &mut ui,
            SceneSource::Entry(&graph),
            &view,
            &library,
            &RunState::default(),
            true,
        );

        // Every node renders, not silently dropped — so the unresolvable ones
        // stay selectable and deletable to repair the document.
        assert_eq!(scene.nodes.len(), 3, "all nodes render");
        let known_node = scene.nodes.get(&known_id).unwrap();
        let ghost_func_node = scene.nodes.get(&ghost_func_id).unwrap();
        let ghost_graph_node = scene.nodes.get(&ghost_graph_id).unwrap();

        // The flag tracks resolution; the label names what's missing.
        assert!(!known_node.missing, "a resolved func is not a stub");
        assert!(ghost_func_node.missing && ghost_graph_node.missing);
        assert_eq!(&*ghost_func_node.kind_label.borrow_str(), "missing func");
        assert_eq!(&*ghost_graph_node.kind_label.borrow_str(), "missing graph");

        // Both stubs keep their saved name and carry no ports — and the
        // graph stub drops its link so "open in tab" is unavailable.
        assert_eq!(&*ghost_func_node.name.borrow_str(), "astro_to_image");
        assert_eq!(&*ghost_graph_node.name.borrow_str(), "removed_graph");
        assert!(ghost_graph_node.graph.is_none());
        for stub in [ghost_func_node, ghost_graph_node] {
            assert_eq!(scene.inputs(stub.inputs).len(), 0, "stub has no inputs");
            assert_eq!(scene.outputs(stub.outputs).len(), 0, "stub has no outputs");
        }

        // The resolved node, by contrast, exposes its real ports.
        assert!(
            !scene.inputs(known_node.inputs).is_empty(),
            "the resolved func still renders its interface"
        );

        // Run seeding follows resolution: the resolved func can be run to,
        // the stubs (and any graph instance) can't.
        assert!(
            known_node.disabled && known_node.runnable(),
            "a resolved disabled func can be targeted by a one-run override"
        );
        assert!(
            !ghost_func_node.runnable() && !ghost_graph_node.runnable(),
            "stubs offer no run affordance — they resolve to nothing"
        );

        scene.rebuild(
            &mut ui,
            SceneSource::Entry(&graph),
            &view,
            &library,
            &RunState::default(),
            false,
        );
        assert!(
            !scene.nodes.get(&known_id).unwrap().runnable(),
            "a local-definition projection hides the run affordance from resolved functions"
        );
    }

    #[test]
    fn func_events_project_in_order_alongside_outputs() {
        use scenarium::{FRAME_EVENT_FUNC_ID, worker_events_library};

        // The `frame event` func declares two events ("Always", "FPS") and two
        // data outputs ("Delta", "Frame #"); the projection must surface both
        // independently — events in their own pool, outputs unchanged.
        let library = worker_events_library();
        let mut graph = Graph::default();
        let node: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
        let node_id = graph.add(node);

        let view = GraphView::for_graph(&graph);
        let mut scene = Scene::default();
        let mut ui = Ui::default();
        scene.rebuild(
            &mut ui,
            SceneSource::Entry(&graph),
            &view,
            &library,
            &RunState::default(),
            true,
        );

        let n = scene.nodes.get(&node_id).unwrap();
        let event_names: Vec<String> = scene
            .events(n.events)
            .iter()
            .map(|e| e.name.borrow_str().to_owned())
            .collect();
        assert_eq!(event_names, ["Always", "FPS"], "events project in order");

        let output_names: Vec<String> = scene
            .outputs(n.outputs)
            .iter()
            .map(|o| o.name.borrow_str().to_owned())
            .collect();
        assert_eq!(
            output_names,
            ["Delta", "Frame #"],
            "data outputs are unaffected by events"
        );
    }

    #[test]
    fn pinned_output_projects_per_output_port_and_shares_the_z_order() {
        use scenarium::{FRAME_EVENT_FUNC_ID, worker_events_library};

        // "frame event" has two data outputs (Delta, Frame #); pin only the
        // second and confirm the flag lands on the right pooled entry, not
        // both or neither.
        let library = worker_events_library();
        let mut graph = Graph::default();
        let node: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
        let node_id = graph.add(node);
        let port = OutputPort::new(node_id, 1);
        graph.set_output_pinned(port, true);

        let mut view = GraphView::for_graph(&graph);
        let pin_key = ItemRef::Pin(port);
        *view.item_placements.get_mut(&pin_key).unwrap() = Vec2::new(320.0, -40.0);
        let mut scene = Scene::default();
        let mut ui = Ui::default();
        scene.rebuild(
            &mut ui,
            SceneSource::Entry(&graph),
            &view,
            &library,
            &RunState::default(),
            true,
        );

        let n = scene.nodes.get(&node_id).unwrap();
        let pins: Vec<Option<Vec2>> = scene
            .outputs(n.outputs)
            .iter()
            .map(|o| o.pin_position)
            .collect();
        assert_eq!(
            pins,
            [None, Some(Vec2::new(320.0, -40.0))],
            "only the pinned port carries a position, projected from its item"
        );

        // The shared paint stack mirrors `item_placements` order — node then
        // pin here (`for_graph` seeds pins after nodes)...
        assert_eq!(
            scene.z_order,
            vec![ItemRef::Node(node_id), pin_key],
            "z_order interleaves node bodies and pin previews in item order"
        );

        // ...and a reorder (pin buried beneath the node) projects verbatim.
        view.move_item_to_index(&pin_key, 0);
        scene.rebuild(
            &mut ui,
            SceneSource::Entry(&graph),
            &view,
            &library,
            &RunState::default(),
            true,
        );
        assert_eq!(
            scene.z_order,
            vec![pin_key, ItemRef::Node(node_id)],
            "restacking the view items restacks the projected z_order"
        );
    }

    #[test]
    fn subscriptions_project_from_graph() {
        use scenarium::{FRAME_EVENT_FUNC_ID, worker_events_library};

        // Two frame-event nodes; subscribe the second to the first's "FPS"
        // event (event_idx 1). The projection must mirror that one edge.
        let library = worker_events_library();
        let mut graph = Graph::default();
        let emitter: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
        let emitter_id = graph.add(emitter);
        let subscriber: Node = library.by_id(FRAME_EVENT_FUNC_ID).unwrap().into();
        let subscriber_id = graph.add(subscriber);
        graph.subscribe(emitter_id, 1, subscriber_id);

        let view = GraphView::for_graph(&graph);
        let mut scene = Scene::default();
        let mut ui = Ui::default();
        scene.rebuild(
            &mut ui,
            SceneSource::Entry(&graph),
            &view,
            &library,
            &RunState::default(),
            true,
        );

        assert_eq!(scene.subscriptions.len(), 1);
        let s = &scene.subscriptions[0];
        assert_eq!(s.emitter, emitter_id);
        assert_eq!(s.event_idx, 1);
        assert_eq!(s.subscriber, subscriber_id);
    }

    #[test]
    fn cache_mode_projects_verbatim_per_node() {
        use scenarium::math_library;

        // One `Add` node per cache mode; each `SceneNode.cache` must mirror its source
        // node's mode exactly (the header reads the two bits off it).
        let library = math_library();
        let mut graph = Graph::default();
        let mut ids = Vec::new();
        for mode in [
            CacheMode::None,
            CacheMode::Ram,
            CacheMode::Disk,
            CacheMode::Both,
        ] {
            let mut node: Node = library.by_name("Add").unwrap().into();
            node.cache = mode;
            let node_id = graph.add(node);
            ids.push((node_id, mode));
        }

        let view = GraphView::for_graph(&graph);
        let mut scene = Scene::default();
        let mut ui = Ui::default();
        scene.rebuild(
            &mut ui,
            SceneSource::Entry(&graph),
            &view,
            &library,
            &RunState::default(),
            true,
        );

        for (id, mode) in ids {
            let projected = scene.nodes.get(&id).unwrap();
            assert_eq!(projected.cache, mode, "{mode:?} projects verbatim");
            assert!(projected.cache_controls);
        }
    }

    #[test]
    fn graph_instances_can_evict_but_have_no_direct_cache_storage_controls() {
        use scenarium::math_library;

        let library = math_library();
        let nested = GraphDef::new("Nested").output(FuncOutput::new("Out", DataType::Int));
        let nested_id = GraphId::unique();
        let mut graph = Graph::default();
        let instance_id = graph.add_graph_node(&nested, GraphLink::Local(nested_id));
        graph.insert_graph(nested_id, nested);

        let view = GraphView::for_graph(&graph);
        let mut scene = Scene::default();
        let mut ui = Ui::default();
        scene.rebuild(
            &mut ui,
            SceneSource::Entry(&graph),
            &view,
            &library,
            &RunState::default(),
            true,
        );

        let instance = &scene.nodes[&instance_id];
        assert!(
            instance.can_evict_cache,
            "an instance can evict its flattened interior"
        );
        assert!(
            !instance.cache_controls,
            "an instance has no runtime slot on which to store an output"
        );
    }

    #[test]
    fn impure_flag_projects_from_func_behavior() {
        use scenarium::{Func, FuncId};

        // Two funcs identical but for behavior: a `Pure` one (offers the disk-cache
        // toggle) and an `Impure` one (has no content digest, so the toggle is hidden).
        // Both have an output, so `impure` is the sole
        // differentiator the header gate reads.
        let mut library = Library::default();
        library.add(testing::with_stub_lambda(
            Func::new("bbebd119-82d8-45cc-a710-cdaa45426521", "pure_src")
                .pure()
                .output(FuncOutput::new("out", DataType::Int)),
        ));
        library.add(testing::with_stub_lambda(
            Func::new("9a97bb06-2c2e-443a-a836-6a11e29cbea7", "impure_src")
                .output(FuncOutput::new("out", DataType::Int)),
        ));
        library.add(testing::with_stub_lambda(
            Func::new(FuncId::unique(), "self_cached")
                .pure()
                .uncacheable()
                .output(FuncOutput::new("out", DataType::Int)),
        ));

        let mut graph = Graph::default();
        let pure_id = graph.add_func_node(library.by_name("pure_src").unwrap());
        let impure_id = graph.add_func_node(library.by_name("impure_src").unwrap());
        let self_cached_id = graph.add_func_node(library.by_name("self_cached").unwrap());

        let view = GraphView::for_graph(&graph);
        let mut scene = Scene::default();
        let mut ui = Ui::default();
        scene.rebuild(
            &mut ui,
            SceneSource::Entry(&graph),
            &view,
            &library,
            &RunState::default(),
            true,
        );

        let pure = scene.nodes.get(&pure_id).unwrap();
        let impure = scene.nodes.get(&impure_id).unwrap();
        let self_cached = scene.nodes.get(&self_cached_id).unwrap();

        assert!(!pure.impure, "a Pure func keeps its cache chips");
        assert!(impure.impure, "an Impure func hides its cache chips");
        // Both have an output, so `impure` is the sole eviction differentiator.
        assert!(
            pure.can_evict_cache,
            "a Pure func with an output can be evicted"
        );
        assert!(!impure.can_evict_cache, "an Impure func cannot be evicted");
        assert!(pure.cache_controls);
        assert!(!impure.cache_controls);
        assert!(
            self_cached.can_evict_cache,
            "self-caching funcs can still have cached downstream consumers"
        );
        assert!(
            !self_cached.cache_controls,
            "self-caching funcs hide Scenarium storage controls"
        );
        assert!(!pure.sink && !impure.sink);
    }
}
