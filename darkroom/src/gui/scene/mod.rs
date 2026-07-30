use std::collections::BTreeSet;

use common::Span;
use glam::Vec2;
use indexmap::IndexMap;
use palantir::{InternedStr, Ui};
use scenarium::Library;
use scenarium::NodePorts;
use scenarium::{Binding, CacheMode, Graph, InputPort, NodeId, NodeKind, OutputPort, Subscription};
use scenarium::{DataType, OutputTypes, RamUsage, StaticValue};
use scenarium::{FuncInput, OutputType, ValueVariant};

use crate::core::document::{Document, GraphView, PortKind, PortRef, Viewport};
use crate::core::preview;
use crate::gui::EventRef;
use crate::gui::run_state::{ExecStatus, RunState};

/// The per-record projection of the graph currently on screen.
///
/// Everything the graph owns lives in a flat pool sliced by a [`Span`] on each
/// [`SceneNode`]: node projections and the per-port pools under them. The
/// steady-state rebuild allocates nothing — every `Vec` retains capacity across
/// the per-frame `clear` + re-`extend`.
///
/// **Only what the projection transforms.** A node's ports carry interned
/// names, wildcard output types resolved through the graph, and the last
/// run's verdicts folded in — work worth doing once a frame rather than per
/// widget. Wiring, selection and placements are *not* here: they are read
/// off the document through [`Pane`], because copying them would buy
/// nothing but a second source of truth to keep in step.
///
/// Reads go through [`Pane`], resolved by [`Scene::pane`].
#[derive(Debug, Default)]
pub(crate) struct Scene {
    /// The paint stack, mirrored from `GraphView::item_placements` order: later
    /// entries drawn in front. The canvas draw pass iterates it; everything
    /// else looks items up through `nodes`.
    z_order: Vec<NodeId>,
    /// Keyed node projections in paint order. Interaction scans use this order
    /// to resolve overlapping node and port hits.
    pub(crate) nodes: IndexMap<NodeId, SceneNode>,
    /// One flat pool of [`SceneInput`] across every node, sliced by the single
    /// `SceneNode::inputs` span. A struct-per-port (not parallel columns) so
    /// the per-port fields can't desync.
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
    /// Output types for the graph being projected, refreshed by
    /// [`Scene::project`]. Not a pool: a lookup table read by
    /// `push_node_ports`, refilled before the nodes are walked, so no frame can
    /// read a type resolved against an older graph.
    output_types: OutputTypes,
}

/// The graph pane for this frame: the projection, and the authoring state it
/// was built from. The read handle every per-pane widget takes. `Copy` (two
/// shared refs), so it threads through the draw chain like `RecordCtx`.
///
/// Both halves, deliberately. A widget reads *rendering* facts — resolved
/// port types, interned names, run status — off the projection, which is
/// where the expensive resolution was done once; it reads *authoring* facts
/// — wiring, selection, placements — off the document, which is where they
/// actually live. A pane carrying only the first half is what made the
/// projection grow verbatim copies of the second.
///
/// Holding one is the proof that a pane *is* showing the graph:
/// [`Scene::pane`] is the only way to obtain one and it checks that once, so
/// no reader has to. A pass that runs whether or not a graph is on screen
/// takes `Option<Pane>` and says so in its signature.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pane<'a> {
    scene: &'a Scene,
    doc: &'a Document,
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
    /// Port tooltip from the func's output declaration; empty when the port
    /// declares none.
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
    pub(crate) pos: Vec2,
    pub(crate) name: InternedStr,
    /// Human-readable type identity: the func's name, or "missing func" for a
    /// stub. Shown by the inspection panel.
    pub(crate) kind_label: InternedStr,
    /// The func's [`Func::description`](scenarium::Func::description) (empty
    /// for a missing stub). Shown by the inspection panel and the new-node
    /// palette tooltip.
    pub(crate) description: InternedStr,
    /// Span into [`Scene::inputs`].
    pub(crate) inputs: Span,
    /// Span into [`Scene::outputs`].
    pub(crate) outputs: Span,
    /// Span into [`Scene::events`]. Listed under the output ports.
    pub(crate) events: Span,
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
    /// Whether the header offers runtime cache eviction for this node — it
    /// needs a reproducible output, which an impure or portless node has not.
    pub(crate) can_evict_cache: bool,
    /// The node holds work that recomputes every run. An impure node has no
    /// content digest, so no cache mode is ever honored (folded into the cache
    /// controls); the header paints the `~` marker off this flag.
    ///
    pub(crate) impure: bool,
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
    /// The node's func is absent from the library (e.g. a document saved
    /// against an older library), so its interface can't be resolved. Rendered
    /// as a portless error stub the user can still select and delete — never
    /// silently dropped.
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

    /// Whether Darkroom exposes the disable toggle for this node. Limiting it
    /// to runnable sinks keeps disabled nodes directly runnable with their
    /// upstream cone intact.
    pub(crate) fn can_disable(&self) -> bool {
        self.sink && !self.missing
    }
}

impl Scene {
    /// The pane showing `doc`'s graph, or `None` when no pane is this frame.
    /// At most one exists — `TabRef::Graph` is a single tab — and holding the
    /// result is the proof that it does, so no reader re-checks.
    ///
    /// Asked of the *document*, not of the pools: a graph with no nodes on an
    /// active tab is a legitimate pane, and one that answered "empty pools, so
    /// no pane" would leave a fresh document with no canvas to place its first
    /// node on. The pools agree by construction — `Editor` rebuilds this
    /// projection after every mutation, so the same predicate gated the fill.
    pub(crate) fn pane<'a>(&'a self, doc: &'a Document) -> Option<Pane<'a>> {
        doc.shows_graph().then_some(Pane { scene: self, doc })
    }

    /// Project the document's graph, replacing the previous pass's contents.
    ///
    /// Names are arena-backed handles authored through this record pass's
    /// `Ui`. Rebuilding keeps the projection synchronized with the document
    /// and lets the previous pass's text arena be recycled. `App::record`
    /// enforces this before widgets consume the scene.
    pub(crate) fn rebuild(
        &mut self,
        ui: &mut Ui,
        library: &Library,
        run_state: &RunState,
        doc: &Document,
    ) {
        self.clear_pools();
        if !doc.shows_graph() {
            return;
        }
        // One handle for the empty string, cloned wherever a port declares no
        // description or a node carries no authored name — see
        // `intern_or_empty`.
        let empty = ui.intern("");
        self.project(ui, library, run_state, doc, &empty);
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
            z_order,
            nodes,
            inputs,
            outputs,
            events,
            value_variants_pool,
            // Not a pool this slices into: a lookup table `project` refreshes
            // per graph.
            output_types: _,
        } = self;
        z_order.clear();
        nodes.clear();
        inputs.clear();
        outputs.clear();
        events.clear();
        value_variants_pool.clear();
    }

    /// Project the document's graph — its nodes, wiring, and view state — into
    /// the pools above.
    fn project(
        &mut self,
        ui: &mut Ui,
        library: &Library,
        run_state: &RunState,
        doc: &Document,
        empty: &InternedStr,
    ) {
        let (graph, view) = (&doc.graph, &doc.main_view);
        // Every wildcard port of this graph, resolved once here rather than
        // walked once per port below.
        self.output_types.update(graph, library);

        for (key, position) in &view.item_placements {
            let id = *key;
            let Some(node) = graph.find(id) else {
                continue;
            };
            // The declaration scenarium resolves for the node — a library func
            // or a special node's hardcoded spec. `None` is a node whose func
            // the library no longer holds: it projects as a portless `missing`
            // stub rather than vanishing, so the user can still select and
            // delete it instead of the document silently losing a node.
            let ports = graph.node_ports(node, library);
            debug_assert!(
                ports.is_some() || matches!(node.kind, NodeKind::Func(_)),
                "a special node's interface always resolves"
            );
            let spans = self.push_node_ports(
                ui,
                library,
                doc,
                id,
                ports,
                run_state.missing_inputs(id),
                empty,
            );
            let name = intern_or_empty(ui, empty, &node.name);
            let previous = self.nodes.insert(
                id,
                SceneNode {
                    id,
                    pos: *position,
                    name,
                    kind_label: ui.intern(ports.map_or(MISSING_FUNC_LABEL, |p| p.name)),
                    description: intern_or_empty(
                        ui,
                        empty,
                        ports.and_then(|p| p.description).unwrap_or_default(),
                    ),
                    inputs: spans.inputs,
                    outputs: spans.outputs,
                    events: spans.events,
                    // The compiled program is the authority once one exists;
                    // before the first compile the declaration's own reading
                    // stands.
                    sink: run_state
                        .is_sink(id)
                        .unwrap_or_else(|| ports.is_some_and(|p| p.sink())),
                    disabled: node.disabled,
                    cache: node.cache,
                    cache_controls: cache_controls(ports),
                    can_evict_cache: can_evict_cache(ports),
                    impure: run_state
                        .is_impure(id)
                        .unwrap_or_else(|| ports.is_some_and(|p| p.impure())),
                    exec_status: run_state.status(id),
                    ram: run_state.ram(id),
                    missing: ports.is_none(),
                    preview: ports.is_some()
                        && matches!(node.kind, NodeKind::Func(func_id) if preview::is_preview(func_id)),
                },
            );
            debug_assert!(previous.is_none(), "node ids are unique within a graph");
            self.z_order.push(*key);
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
        doc: &Document,
        id: NodeId,
        ports: Option<NodePorts<'_>>,
        missing_inputs: &[usize],
        empty: &InternedStr,
    ) -> NodePortSpans {
        let graph = &doc.graph;
        // One `SceneInput` per input port. Each input's value_variants are
        // flattened into the shared pool, the input recording its span
        // (empty for the common no-options case) — so this one can't go
        // through `extend_pool`, which would borrow two pools at once.
        let inputs_start = self.inputs.len();
        for (port_idx, input) in declared(ports, |p| p.inputs).iter().enumerate() {
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
        // Shared borrow of the table `project` filled, so reading a port's type
        // and filling the output pool stay two disjoint borrows of `self`.
        let output_types = &self.output_types;
        let outputs = extend_pool(
            &mut self.outputs,
            declared(ports, |p| p.outputs)
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
                        // `project` resolved every wildcard port of this
                        // graph before walking its nodes, and only a node whose
                        // interface came from `Graph::node_ports` — the same set
                        // `update` walks — can declare one. A miss is that
                        // invariant broken, and defaulting it would paint the
                        // port `Any`, which accepts *any* connection.
                        OutputType::Wildcard { .. } => output_types
                            .get(OutputPort::new(id, i))
                            .expect("every wildcard port of the projected graph is resolved")
                            .clone(),
                        OutputType::Fixed(dt) => dt.clone(),
                    },
                }),
        );
        let events = extend_pool(
            &mut self.events,
            declared(ports, |p| p.events)
                .iter()
                .map(|event| SceneEvent {
                    name: ui.intern(&event.name),
                }),
        );
        NodePortSpans {
            inputs,
            outputs,
            events,
        }
    }
}

impl<'a> Pane<'a> {
    /// The whole-scene projection behind this pane. For the sweeps keyed by
    /// document-unique ids — `CanvasGeometry`'s rebuild, a drag's
    /// owner-still-alive check — which walk every node rather than asking
    /// about one.
    pub(crate) fn scene(self) -> &'a Scene {
        self.scene
    }

    /// The authoring graph this pane shows.
    pub(crate) fn body(self) -> &'a Graph {
        &self.doc.graph
    }

    /// Its view metadata: placements, viewport, committed selection.
    pub(crate) fn view(self) -> &'a GraphView {
        &self.doc.main_view
    }

    pub(crate) fn viewport(self) -> Viewport {
        self.doc.main_view.viewport
    }

    /// Whether `node` can seed a "run to this node" — drives the header play
    /// chip, the context-menu item, and the port menu's "Add preview".
    ///
    /// Everything but a `missing` stub qualifies: the stub resolves to no
    /// compiled work, while a *disabled* node still runs, because a targeted
    /// run overrides that flag for the run. Deliberately an authoring-side
    /// fact, not a lookup in a compiled program — the palette and the header
    /// record every frame, including before the first compile, so an
    /// affordance can't wait on one.
    ///
    /// A method on the pane, not the node: "is a run targetable here" is a
    /// fact about the pane, and every caller already has one in hand.
    pub(crate) fn runnable(self, node: &SceneNode) -> bool {
        !node.missing
    }

    /// This graph's nodes, in relative paint order.
    pub(crate) fn nodes(self) -> impl Iterator<Item = &'a SceneNode> {
        self.scene.nodes.values()
    }

    /// A node of *this* graph by id. Filtered by owner, so a pane never
    /// resolves an id belonging to another open pane.
    pub(crate) fn node(self, node_id: NodeId) -> Option<&'a SceneNode> {
        self.scene.nodes.get(&node_id)
    }

    pub(crate) fn contains(self, node_id: NodeId) -> bool {
        self.node(node_id).is_some()
    }

    /// This graph's paint stack: node bodies
    /// interleaved, later entries in front.
    pub(crate) fn z_order(self) -> &'a [NodeId] {
        &self.scene.z_order
    }

    /// This graph's data edges, as `(consumer input ← producer output)`.
    /// Read off the authoring graph: the projection would only be holding a
    /// copy of it.
    pub(crate) fn connections(self) -> impl Iterator<Item = (InputPort, OutputPort)> + 'a {
        self.body().edges()
    }

    /// This graph's event-subscription edges, likewise straight off the
    /// authoring graph.
    pub(crate) fn subscriptions(self) -> impl Iterator<Item = Subscription> + 'a {
        self.body().subscriptions()
    }

    /// This graph's committed selection.
    pub(crate) fn selected(self) -> &'a BTreeSet<NodeId> {
        &self.view().selected
    }

    /// Whether `key` is in this graph's committed selection.
    pub(crate) fn is_selected(self, key: NodeId) -> bool {
        self.view().selected.contains(&key)
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

fn slice_pool<T>(pool: &[T], span: Span) -> &[T] {
    &pool[span.range()]
}

/// The [`Span`] covering `start..end` of a pool.
fn span_since(start: usize, end: usize) -> Span {
    Span::new(start as u32, (end - start) as u32)
}

/// The `kind_label` a node projects with when the library holds no func for it
/// — see [`SceneNode::missing`].
const MISSING_FUNC_LABEL: &str = "missing func";

/// One port list off a node's declaration, empty for a `missing` stub. The stub
/// declares nothing, so every pool it contributes to is empty rather than
/// special-cased.
fn declared<'a, T>(
    ports: Option<NodePorts<'a>>,
    which: impl Fn(&NodePorts<'a>) -> &'a [T],
) -> &'a [T] {
    ports.map_or(&[][..], |p| which(&p))
}

/// Whether the header offers the RAM/disk storage chips — see
/// [`SceneNode::cache_controls`]. An impure func has no content digest to key a
/// cache on, and a func that declares itself uncacheable or exposes no outputs
/// has nothing to store — a `missing` stub for both reasons at once.
fn cache_controls(ports: Option<NodePorts<'_>>) -> bool {
    ports.is_some_and(|p| !p.uncacheable() && !p.outputs.is_empty() && !p.impure())
}

/// Whether the header offers runtime cache eviction — see
/// [`SceneNode::can_evict_cache`]. Needs a reproducible output, which rules out
/// impure funcs and portless nodes.
fn can_evict_cache(ports: Option<NodePorts<'_>>) -> bool {
    ports.is_some_and(|p| !p.outputs.is_empty() && !p.impure())
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
            pos,
            name: ui.intern(""),
            kind_label: ui.intern(""),
            description: ui.intern(""),
            inputs: Span::default(),
            outputs: Span::default(),
            events: Span::default(),
            sink: false,
            disabled: false,
            cache: CacheMode::None,
            cache_controls: false,
            can_evict_cache: false,
            impure: false,
            exec_status: ExecStatus::None,
            ram: RamUsage::default(),
            missing: false,
            preview: false,
        }
    }

    /// A sealed one-pane projection over a hand-built node set, plus the
    /// document its [`Pane`] resolves against. The harness every canvas test
    /// that needs a `Pane` builds on.
    #[derive(Debug, Default)]
    pub(crate) struct SceneFixture {
        pub(crate) scene: Scene,
        pub(crate) doc: Document,
    }

    impl SceneFixture {
        pub(crate) fn with_nodes(nodes: impl IntoIterator<Item = SceneNode>) -> Self {
            let mut fixture = SceneFixture::default();
            for node in nodes {
                fixture.scene.z_order.push(node.id);
                fixture.scene.nodes.insert(node.id, node);
            }
            fixture
        }

        /// Give the sole pane a committed selection.
        pub(crate) fn with_selection(mut self, selected: impl IntoIterator<Item = NodeId>) -> Self {
            self.doc.main_view.selected.extend(selected);
            self
        }

        /// The fixture's sole pane.
        pub(crate) fn only_pane(&self) -> Pane<'_> {
            self.pane().expect("the fixture seals one pane")
        }

        pub(crate) fn pane(&self) -> Option<Pane<'_>> {
            self.scene.pane(&self.doc)
        }
    }
}

#[cfg(test)]
mod tests;
