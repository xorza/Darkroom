//! One node of the graph, with its declaration resolved.

use glam::Vec2;
use scenarium::{
    CacheMode, CompoundId, DetachReason, EventDecl, EventPort, EventRef, Func, GhostNode,
    GhostSource, GhostWiring, GraphId, InputPort, Node, NodeAddr, NodeDecl, NodeEvents, NodeId,
    NodeTriggers, Numbered, PortId, ProgramId, RamUsage, SavedDecl, TriggerPort, Unresolved,
};

use crate::core::document;
use crate::core::preview_sink;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::ghost_port::{GhostEvent, GhostPort, GhostTrigger};
use crate::gui::graph_ctx::input_ctx::InputCtx;
use crate::gui::graph_ctx::output_ctx::OutputCtx;
use crate::gui::port_ref::{PortKind, PortRef};
use crate::gui::state::run_state::{ExecStatus, InstanceRunState, NodeRunState};
use crate::gui::theme::Theme;
use crate::gui::theme::const_editor_theme::ConstEditorTheme;
use crate::gui::theme::hover::Hover;

/// One placed item of the graph pane — a graph node or a ghost node: its authoring
/// record, its position in the paint stack, and what it declares.
///
/// The leaf of the context chain, where the *item* is the level: every function below a node
/// body is about that node.
///
/// **Available for the whole frame**: a [`GraphCtx`] is all it takes to resolve one, so the hit
/// sweep, the input stage's gestures and the record all read nodes through this type. The facts
/// only a *record* has settled — the effective selection and the cull region — stay on
/// [`DrawCtx`](crate::gui::pane::graph_ui::draw_ctx::DrawCtx), so a reader cannot reach for one
/// in a phase where it does not exist yet.
///
/// `Copy`, and the borrows are taken once at [`Self::resolve`]: one library lookup per node, not
/// one per field read.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NodeCtx<'a> {
    pub(crate) graph_ctx: GraphCtx<'a>,
    pub(crate) node_id: NodeId,
    /// Where the node's body sits, from the view's placements.
    pub(crate) pos: Vec2,
    /// The graph node, or a ghost node's record of one.
    node: &'a Node,
    /// The ghost node this is, for what it declared when the load took it out.
    ghost_node: Option<&'a GhostNode>,
    /// What a graph node declares — a library func, a special node's hardcoded spec, or a
    /// compound's face — or why the load kept a ghost node. A ghost node keeps the user's work while its
    /// graph cannot hold the node: it declares no ports, and the user can still select, move and
    /// delete it.
    declaration: Result<NodeDecl<'a>, DetachReason>,
    /// What the last run said about this node, looked up once at resolve.
    run: RunView<'a>,
    /// Whether the pointer is over this node's body — [`Hover::Rest`] until a
    /// caller that holds a `Ui` sets it through [`Self::with_hover`].
    ///
    /// Drives the const editors' hover-revealed chips ([`Self::const_editor_theme`]) and whether
    /// the port rows build tooltips ([`Self::tooltips`]): one field, as the two are one fact.
    ///
    /// Not resolved in [`Self::resolve`], which takes no `Ui`; the record pass sets it once per
    /// node body.
    hover: Hover,
}

/// The program node `node` of `graph_ctx`'s graph is in the instance the pane shows: `None` when
/// it shows none, or for a node with no body, an instance or a boundary node, which emits none.
fn program_id(graph_ctx: GraphCtx<'_>, node_id: NodeId, node: &Node) -> Option<ProgramId> {
    graph_ctx.program_id(node_id).filter(|_| node.has_body())
}

/// What the last run said about one node of a pane.
#[derive(Clone, Copy, Debug)]
enum RunView<'a> {
    /// Nothing: the run reported nothing of it, or the pane shows no instance.
    None,
    /// A node with a body, as its program node.
    Node(&'a NodeRunState),
    /// An instance, as the nodes inside it.
    Instance(&'a InstanceRunState),
}

impl<'a> NodeCtx<'a> {
    /// Resolve the placed `node_id` against the document.
    ///
    /// # Panics
    ///
    /// If the document holds no `node_id`: callers resolve placements, and the
    /// document holds a placement exactly for every graph node and ghost node.
    pub(super) fn resolve(graph_ctx: GraphCtx<'a>, node_id: NodeId, pos: Vec2) -> Self {
        let (node, declaration, ghost_node) = if let Some(node) = graph_ctx.body().find(node_id) {
            (node, Ok(node.decl(graph_ctx.scope())), None)
        } else {
            let ghost = graph_ctx
                .body()
                .ghost_node(node_id)
                .expect("every placement names a graph node or a ghost node");
            (&ghost.node, Err(ghost.reason), Some(ghost))
        };
        let run_state = graph_ctx.run_state();
        let program_id = program_id(graph_ctx, node_id, node);
        let run = match (program_id, node.compound(), graph_ctx.through()) {
            (Some(id), _, _) => run_state.node(id).map_or(RunView::None, RunView::Node),
            // An instance shows what the nodes inside it came to.
            (None, Some(_), Some(path)) => run_state
                .instance(path.child(node_id))
                .map_or(RunView::None, RunView::Instance),
            _ => RunView::None,
        };
        Self {
            graph_ctx,
            node_id,
            pos,
            node,
            ghost_node,
            declaration,
            run,
            hover: Hover::Rest,
        }
    }

    /// This node with the pointer-over question answered, resolved once by the
    /// record pass for everything below the node body that asks it.
    ///
    /// Takes the answer rather than a `Ui`: the widget id it comes off is
    /// `gui::pane::graph_ui::wid::hover`'s, and this type sits below
    /// that module.
    pub(crate) const fn with_hover(self, hover: Hover) -> Self {
        Self { hover, ..self }
    }

    /// Whether the port rows build their hover tooltips: composed per port per frame, so
    /// only the node under the pointer pays.
    pub(crate) const fn tooltips(self) -> bool {
        matches!(self.hover, Hover::Over)
    }

    /// The const-editor styling this node's value cells paint with: the pointer over the node
    /// surfaces the resting boxes further, so the edit affordance grows where the pointer is and
    /// geometry never changes.
    pub(crate) const fn const_editor_theme(self) -> &'a ConstEditorTheme {
        self.graph_ctx.theme().const_editor.at(self.hover)
    }

    /// The palette and metrics this node paints from.
    pub(crate) const fn theme(self) -> &'a Theme {
        self.graph_ctx.theme()
    }

    /// The node's title ([`document::title`]): its name, or an unnamed instance's compound's;
    /// empty for any other node that carries none.
    pub(crate) fn name(self) -> &'a str {
        document::title(self.node, self.graph_ctx.scope())
    }

    /// Human-readable type identity: the declaration's name, or for a ghost node why the load took it
    /// out — see [`NodeCtx::ghost`]. Shown by the inspector pane.
    pub(crate) fn kind_label(self) -> &'a str {
        match self.declaration {
            Ok(decl) => decl.name(),
            Err(DetachReason::Unresolved(Unresolved::MissingFunc)) => "missing func",
            Err(DetachReason::Unresolved(Unresolved::MissingCompound)) => "missing compound",
            Err(DetachReason::Unresolved(Unresolved::NoInterface)) => "boundary outside a compound",
            Err(DetachReason::ExtraOutputs) => "extra Outputs node",
            Err(DetachReason::Recursive) => "compound inside itself",
            Err(DetachReason::TooDeep) => "nested too deep",
            Err(DetachReason::TooLarge) => "program too large",
        }
    }

    /// The declaration's description, empty for a ghost node or a declaration that has none.
    /// Shown by the inspector pane and the new-node palette tooltip.
    pub(crate) fn description(self) -> &'a str {
        self.declaration
            .ok()
            .and_then(NodeDecl::description)
            .unwrap_or_default()
    }

    /// A ghost node: a node the library cannot resolve, kept beside the graph.
    /// Rendered as a portless error card the user can still select, move and
    /// delete — never silently dropped.
    pub(crate) const fn ghost(self) -> bool {
        self.declaration.is_err()
    }

    /// What a ghost node declared when the load took it out, where a save kept it: its func's
    /// name and category, and the ports it shows.
    pub(crate) const fn saved(self) -> Option<&'a SavedDecl> {
        match self.ghost_node {
            Some(ghost) => ghost.saved.as_ref(),
            None => None,
        }
    }

    /// The ghosts this node's graph keeps.
    const fn ghost_wiring(self) -> &'a GhostWiring {
        self.graph_ctx.body().ghost_wiring()
    }

    /// The inputs this node shows that its declaration does not, in the order it shows them: a
    /// ghost node's saved inputs, then each input a ghost sits on that neither a declaration nor
    /// a saved one gives.
    pub(crate) fn ghost_inputs(self) -> impl Iterator<Item = GhostPort<'a>> {
        let (node_id, wiring, saved) = (self.node_id, self.ghost_wiring(), self.saved());
        let declaration = self.declaration;
        let known = move |port: PortId| {
            declaration.is_ok_and(|decl| decl.ports().input(port).is_some())
                || saved.is_some_and(|saved| saved.input(port).is_some())
        };
        let from_saved = saved
            .map_or(&[][..], |saved| &saved.inputs)
            .iter()
            .map(move |input| GhostPort {
                port: PortRef::input(node_id, input.id),
                name: Some(input.name.as_str()),
                data_type: Some(input.data_type),
                held: wiring.binding(InputPort::new(node_id, input.id)),
            });
        let unknown = wiring
            .bindings_on(node_id)
            .filter(move |(port, _)| !known(port.port))
            .map(|(port, ghost)| GhostPort {
                port: PortRef::from(port),
                name: ghost.input.as_ref().map(|saved| saved.name.as_str()),
                data_type: ghost.input.as_ref().map(|saved| saved.data_type),
                held: Some(ghost),
            });
        from_saved.chain(unknown)
    }

    /// The outputs this node shows that its declaration does not; [`Self::ghost_inputs`] for an
    /// output, each once however many ghost wires read it.
    pub(crate) fn ghost_outputs(self) -> impl Iterator<Item = GhostPort<'a>> {
        let (node_id, wiring, saved) = (self.node_id, self.ghost_wiring(), self.saved());
        let declaration = self.declaration;
        let known = move |port: PortId| {
            declaration.is_ok_and(|decl| decl.ports().output(port).is_some())
                || saved.is_some_and(|saved| saved.output(port).is_some())
        };
        let from_saved = saved
            .map_or(&[][..], |saved| &saved.outputs)
            .iter()
            .map(move |output| GhostPort {
                port: PortRef::output(node_id, output.id),
                name: Some(output.name.as_str()),
                data_type: Some(output.data_type),
                held: None,
            });
        let unknown = wiring.bindings().filter_map(move |(input, ghost)| {
            let GhostSource::Wire {
                output,
                saved: saved_port,
            } = &ghost.source
            else {
                return None;
            };
            if output.node_id != node_id || known(output.port) {
                return None;
            }
            // The first ghost wire that reads the output shows it.
            let shown = wiring
                .bindings()
                .take_while(|(earlier, _)| *earlier < input)
                .any(|(_, earlier)| earlier.output() == Some(*output));
            (!shown).then(|| GhostPort {
                port: PortRef::from(*output),
                name: saved_port.as_ref().map(|saved| saved.name.as_str()),
                data_type: saved_port.as_ref().map(|saved| saved.data_type),
                held: None,
            })
        });
        from_saved.chain(unknown)
    }

    /// The events this node shows that its declaration does not fire; [`Self::ghost_inputs`] for
    /// an event, each once however many ghost subscriptions leave it.
    pub(crate) fn ghost_events(self) -> impl Iterator<Item = GhostEvent<'a>> {
        let (node_id, wiring, saved) = (self.node_id, self.ghost_wiring(), self.saved());
        let declaration = self.declaration;
        let known = move |event: PortId| {
            declaration.is_ok_and(|decl| decl.event(event).is_some())
                || saved.is_some_and(|saved| saved.event(event).is_some())
        };
        let from_saved = saved
            .map_or(&[][..], |saved| &saved.events)
            .iter()
            .map(move |event| GhostEvent {
                event: EventPort::new(node_id, event.id),
                name: Some(event.name.as_str()),
            });
        let unknown = wiring
            .subscriptions_from(node_id)
            .filter(move |(subscription, _)| !known(subscription.event.event))
            .filter(move |(subscription, _)| {
                // The first ghost subscription to the event shows it.
                !wiring
                    .subscriptions_from(node_id)
                    .take_while(|(earlier, _)| earlier < subscription)
                    .any(|(earlier, _)| earlier.event == subscription.event)
            })
            .map(|(subscription, ghost)| GhostEvent {
                event: subscription.event,
                name: ghost.event.as_ref().map(|saved| saved.name.as_str()),
            });
        from_saved.chain(unknown)
    }

    /// The triggers this node lists that its declaration does not show: each `Outputs` trigger a
    /// ghost subscription names whose interface event is gone, each once, under the name the
    /// interface change saved. A node's own lost trigger shows on its header instead
    /// ([`Self::ghost_triggered`]); no other node keeps a ghost on another trigger, as the load
    /// drops it.
    pub(crate) fn ghost_triggers(self) -> impl Iterator<Item = GhostTrigger<'a>> {
        let (node_id, wiring, triggers) = (self.node_id, self.ghost_wiring(), self.triggers());
        // Only the `Outputs` node lists triggers, so every other node walks no ghost wiring.
        let rows = matches!(triggers, NodeTriggers::PerEvent(_));
        let to_node = move || {
            rows.then(|| wiring.subscriptions())
                .into_iter()
                .flatten()
                .filter(move |(subscription, _)| subscription.trigger.node_id == node_id)
        };
        to_node()
            .enumerate()
            .filter(move |(_, (subscription, _))| !triggers.contains(subscription.trigger.trigger))
            .filter(move |&(idx, (subscription, _))| {
                // The first ghost subscription to the trigger shows it.
                !to_node()
                    .take(idx)
                    .any(|(earlier, _)| earlier.trigger == subscription.trigger)
            })
            .map(|(_, (subscription, ghost))| GhostTrigger {
                trigger: subscription.trigger,
                name: ghost.trigger.as_ref().map(|saved| saved.name.as_str()),
            })
    }

    /// Every ghost trigger the node draws, its own on its header and its ghost rows':
    /// [`Self::trigger_ports`] for a ghost subscription's end.
    pub(crate) fn ghost_trigger_ports(self) -> impl Iterator<Item = TriggerPort> {
        let own = self
            .ghost_triggered()
            .then(|| TriggerPort::from(self.node_id));
        own.into_iter()
            .chain(self.ghost_triggers().map(|ghost| ghost.trigger))
    }

    /// Whether the node draws a ghost trigger: a ghost subscription ends on its own trigger and
    /// it shows no trigger, as a ghost node, a node whose func no longer runs as a sink, or an
    /// instance whose interface lost its trigger. Never on the `Outputs` node, whose trigger ids
    /// are its rows'. Read off the wiring, as no saved declaration says sink.
    pub(crate) fn ghost_triggered(self) -> bool {
        let own = TriggerPort::from(self.node_id);
        matches!(self.triggers(), NodeTriggers::None)
            && self
                .ghost_wiring()
                .subscriptions()
                .any(|(subscription, _)| subscription.trigger == own)
    }

    /// How many ghosts touch this node, from either end: the count its footer and the inspector
    /// show.
    pub(crate) fn ghost_count(self) -> usize {
        let wiring = self.ghost_wiring();
        if wiring.is_empty() {
            return 0;
        }
        wiring.count_touching(self.node_id)
    }

    /// Excluded from execution (`Node::disabled`). Sink headers expose the
    /// toggle; the body paints any authored disabled node dimmed.
    pub(crate) const fn disabled(self) -> bool {
        self.node.disabled
    }

    /// Where this node's output is cached, or an instance's outputs, or `None` for a boundary
    /// node. The header's two storage chips toggle its RAM and disk bits.
    pub(crate) const fn cache(self) -> Option<CacheMode> {
        self.node.cache()
    }

    /// Sink node — no outputs feed downstream, so a run has to reach it
    /// explicitly.
    ///
    /// Read off the declaration, so the disable toggle it gates renders before the first
    /// compile.
    pub(crate) fn sink(self) -> bool {
        self.func().is_some_and(Func::sink)
    }

    /// The triggers the node shows ([`NodeDecl::triggers`]). Declared, like [`Self::sink`].
    fn triggers(self) -> NodeTriggers<'a> {
        self.declaration
            .map_or(NodeTriggers::None, NodeDecl::triggers)
    }

    /// The node has its own trigger, drawn on its header: a sink, or an instance whose interface
    /// declares a trigger.
    pub(crate) fn has_own_trigger(self) -> bool {
        matches!(self.triggers(), NodeTriggers::Own)
    }

    /// The node holds work that recomputes every run. An impure node has no
    /// content digest, so no cache mode is ever honored; the header paints
    /// its impure icon off this. Declared, like [`Self::sink`].
    pub(crate) fn impure(self) -> bool {
        self.func().is_some_and(Func::impure)
    }

    /// A preview node: its body shows the value wired into it instead of the
    /// usual output ports and memory readout. A ghost node is never one, so a
    /// document whose library lost the func shows an ordinary ghost node card rather
    /// than an empty preview.
    pub(crate) fn preview(self) -> bool {
        !self.ghost() && self.node.func_id().is_some_and(preview_sink::is_func)
    }

    /// Whether the node has an output a cache can key: the one rule behind
    /// the header's RAM/disk storage chips and its eviction chip. An impure
    /// func has no content digest to key a cache on, and a node with no
    /// outputs has nothing to store; a ghost node and a boundary node have no
    /// cache mode. An instance caches the values that leave it.
    pub(crate) fn cacheable(self) -> bool {
        let Ok(decl) = self.declaration else {
            return false;
        };
        self.cache().is_some()
            && !decl.outputs().is_empty()
            && !self.func().is_some_and(Func::impure)
    }

    /// Whether Darkroom exposes the disable toggle. Limiting it to runnable
    /// sinks keeps disabled nodes directly runnable with their upstream cone
    /// intact.
    pub(crate) fn can_disable(self) -> bool {
        self.sink()
    }

    /// The compound this node places: an instance's; `None` for any other node, and for a ghost
    /// node, whose compound is gone.
    pub(crate) const fn compound(self) -> Option<CompoundId> {
        if self.ghost() {
            return None;
        }
        self.node.compound()
    }

    /// The compound whose interface this node shows: an instance's, or the compound a boundary
    /// node is in; `None` for any other node, and for a ghost node.
    pub(crate) const fn shows_interface(self) -> Option<CompoundId> {
        if self.ghost() {
            return None;
        }
        match (
            self.node.compound(),
            self.node.boundary(),
            self.graph_ctx.graph(),
        ) {
            (Some(id), _, _) | (None, Some(_), GraphId::Compound(id)) => Some(id),
            _ => None,
        }
    }

    /// This node, named in the document.
    pub(crate) const fn addr(self) -> NodeAddr {
        NodeAddr::new(self.graph_ctx.graph(), self.node_id)
    }

    /// The program node this node is in the instance the pane shows, or `None` when it shows
    /// none or the node emits none: what a run, a preview and a viewer of this node name.
    /// Derived on each ask rather than held, as a path is 129 bytes and the context is copied
    /// into every port's.
    pub(crate) fn program_id(self) -> Option<ProgramId> {
        program_id(self.graph_ctx, self.node_id, self.node)
    }

    /// The program node this node can seed a "run to this node" as — drives the header play
    /// chip and the context-menu item — or `None` when it can seed none.
    ///
    /// A node with a body qualifies, shown through an instance: a *disabled* node still runs,
    /// as a targeted run overrides that flag. An authoring-side fact, not a lookup in a compiled
    /// program, so it holds before the first compile.
    pub(crate) fn runnable(self) -> Option<ProgramId> {
        self.program_id().filter(|_| !self.ghost())
    }

    /// Outcome of the last graph run, with its detail. Drives the node's
    /// status frame, the footer's status, the inspector's status and
    /// error lines, and the highlight on the ports the run could not feed.
    ///
    /// An instance's is what the nodes inside it came to: see [`InstanceRunState`].
    pub(crate) const fn exec_status(self) -> &'a ExecStatus {
        match self.run {
            RunView::None => &ExecStatus::None,
            RunView::Node(node) => node.status(),
            RunView::Instance(instance) => instance.status(),
        }
    }

    /// The name of the node inside this instance whose failure is its status, or `None` for a
    /// node that is no instance, or an instance nothing inside failed.
    pub(crate) fn failed_inside(self) -> Option<&'a str> {
        let RunView::Instance(instance) = self.run else {
            return None;
        };
        let failed = instance.failed()?;
        let graphs = &self.graph_ctx.document().graphs;
        let graph = graphs.graph_at(failed.path())?;
        let node = graphs.graph(graph).find(failed.node_id())?;
        Some(document::title(
            node,
            graphs.scope(self.graph_ctx.library(), graph),
        ))
    }

    /// RAM this node's cached output currently holds (system vs GPU).
    /// Non-zero only for nodes that retain a value; drives the node body's
    /// memory readout, hidden when zero.
    pub(crate) fn ram(self) -> RamUsage {
        match self.run {
            RunView::Node(node) => node.ram(),
            RunView::None | RunView::Instance(_) => RamUsage::default(),
        }
    }

    /// This node's input ports, in declaration order. Empty for a ghost node, which
    /// declares none.
    pub(crate) fn inputs(self) -> impl ExactSizeIterator<Item = InputCtx<'a>> {
        let declared = self.declaration.map_or(&[][..], NodeDecl::inputs);
        declared
            .iter()
            .enumerate()
            .map(move |(idx, input)| InputCtx::new(self, idx, input))
    }

    /// The func this node runs, `None` for a ghost node and a node with no body.
    pub(super) fn func(self) -> Option<&'a Func> {
        self.declaration.ok().and_then(NodeDecl::func)
    }

    /// The input port under `port`, if this node declares one.
    pub(crate) fn input(self, port: PortId) -> Option<InputCtx<'a>> {
        let ports = self.declaration.ok()?.ports();
        let idx = ports.input_idx(port)?;
        Some(InputCtx::new(self, idx, &ports.inputs[idx]))
    }

    /// The output port under `port`, if this node declares one.
    pub(crate) fn output(self, port: PortId) -> Option<OutputCtx<'a>> {
        let ports = self.declaration.ok()?.ports();
        let idx = ports.output_idx(port)?;
        Some(OutputCtx::new(self, idx, &ports.outputs[idx]))
    }

    /// This node's output ports, in declaration order.
    pub(crate) fn outputs(self) -> impl ExactSizeIterator<Item = OutputCtx<'a>> {
        let declared = self.declaration.map_or(&[][..], NodeDecl::outputs);
        declared
            .iter()
            .enumerate()
            .map(move |(idx, output)| OutputCtx::new(self, idx, output))
    }

    /// This node's event (emitter) ports. Events carry no data type — they
    /// are pure triggers — so the declaration is all the UI needs, and the
    /// output column lists them under the data outputs.
    pub(crate) fn events(self) -> NodeEvents<'a> {
        self.declaration
            .map_or(NodeEvents::Declared(&[]), NodeDecl::events)
    }

    /// The event under `event`, if this node fires one.
    pub(crate) fn event(self, event: PortId) -> Option<EventRef<'a>> {
        self.declaration.ok()?.event(event)
    }

    /// The triggers the node lists as rows: the `Outputs` node's, one per interface event, in the
    /// input column under the data inputs.
    pub(crate) fn trigger_rows(self) -> &'a [Numbered<EventDecl>] {
        match self.triggers() {
            NodeTriggers::PerEvent(events) => events,
            NodeTriggers::None | NodeTriggers::Own => &[],
        }
    }

    /// Every `TriggerPort` the node shows, its own or its rows': [`Self::event_ports`] for a
    /// subscription's other end.
    pub(crate) fn trigger_ports(self) -> impl Iterator<Item = TriggerPort> {
        let node_id = self.node_id;
        let own = self.has_own_trigger().then(|| TriggerPort::from(node_id));
        own.into_iter().chain(
            self.trigger_rows()
                .iter()
                .map(move |trigger| TriggerPort::new(node_id, trigger.id)),
        )
    }

    /// How many ports this node declares on `kind`'s side.
    pub(crate) fn port_count(self, kind: PortKind) -> usize {
        match kind {
            PortKind::Input => self.declaration.map_or(0, |decl| decl.inputs().len()),
            PortKind::Output => self.declaration.map_or(0, |decl| decl.outputs().len()),
        }
    }

    /// Every `PortRef` on the given side, in declaration order. Single source for
    /// the "iterate a node's ports by kind" loop `CanvasGeometry::rebuild`
    /// and the wire scans all need, so scan order and paint order
    /// can't drift apart.
    pub(crate) fn ports(self, kind: PortKind) -> impl Iterator<Item = PortRef> {
        let node_id = self.node_id;
        let (inputs, outputs) = match (self.declaration, kind) {
            (Err(_), _) => (&[][..], &[][..]),
            (Ok(decl), PortKind::Input) => (decl.inputs(), &[][..]),
            (Ok(decl), PortKind::Output) => (&[][..], decl.outputs()),
        };
        inputs
            .iter()
            .map(|input| input.id)
            .chain(outputs.iter().map(|output| output.id))
            .map(move |port| PortRef {
                node_id,
                kind,
                port,
            })
    }

    /// Every `EventPort`, in declaration order — the emitter-glyph
    /// counterpart of [`Self::ports`], shared by `CanvasGeometry::rebuild`
    /// and the subscription-wire scans for the same reason.
    pub(crate) fn event_ports(self) -> impl Iterator<Item = EventPort> {
        let node_id = self.node_id;
        self.events()
            .iter()
            .map(move |event| EventPort::new(node_id, event.id))
    }

    /// The port indices the last run could not feed — its own verdict, so a
    /// port wired to a disabled or itself-unfed producer counts too, not just
    /// an unbound one.
    pub(super) fn missing_inputs(self) -> &'a [usize] {
        match self.exec_status() {
            ExecStatus::MissingInputs(ports) => ports,
            _ => &[],
        }
    }
}
