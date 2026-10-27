//! One node of the graph, with its declaration resolved.

use glam::Vec2;
use scenarium::{CacheMode, EventPort, Func, FuncEvent, Node, NodeId, RamUsage, Unresolved};

use crate::core::preview_sink;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::input_ctx::InputCtx;
use crate::gui::graph_ctx::output_ctx::OutputCtx;
use crate::gui::port_ref::{PortKind, PortRef};
use crate::gui::state::run_state::{ExecStatus, NodeRunState};
use crate::gui::theme::Theme;
use crate::gui::theme::const_editor_theme::ConstEditorTheme;
use crate::gui::widgets::hover::Hover;

/// The `kind_label` a stub reports when the library holds no func under its
/// id — see [`NodeCtx::stub`].
const MISSING_FUNC_LABEL: &str = "missing func";

/// The `kind_label` a stub reports when the library holds its func with other
/// ports than the node was authored against.
const CHANGED_FUNC_LABEL: &str = "changed func";

/// What a placed item declares: the func a graph node instantiates, or why a
/// stub has none.
#[derive(Clone, Copy, Debug)]
enum Declaration<'a> {
    Func(&'a Func),
    Stub(Unresolved),
}

/// One placed item of the graph pane — a graph node or a stub: its authoring
/// record, its position in the paint stack, and what it declares.
///
/// The leaf of the context chain, and the one level where the *item* is the
/// level — every function below a node body is about that node, so passing it
/// beside the context would be the same argument twice.
///
/// **Available for the whole frame**, unlike the passes above it: a
/// [`GraphCtx`] is all it takes to resolve one, so the hit sweep at frame top,
/// the prepass gestures, and the record all read nodes through this same type.
/// The facts that only a *record* has settled — the effective selection and
/// the cull region — stay on
/// [`DrawCtx`](crate::gui::pane::graph_ui::draw_ctx::DrawCtx), which travels beside
/// this rather than inside it, so a reader cannot reach for one in a phase
/// where it does not exist yet.
///
/// `Copy`, and the three borrows are taken once at [`Self::resolve`] — so a
/// pass that threads one down a widget tree pays a single library lookup for
/// the node, not one per field it reads.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NodeCtx<'a> {
    pub(crate) graph_ctx: GraphCtx<'a>,
    pub(crate) node_id: NodeId,
    /// Where the node's body sits, from the view's placements. A field rather
    /// than an accessor: the paint stack is walked by position, and every
    /// geometry reader wants it.
    pub(crate) pos: Vec2,
    /// The graph node, or a stub's record of one.
    node: &'a Node,
    /// A graph node's func — a library func or a special node's hardcoded
    /// spec — or the reason a stub has none. A stub keeps the user's work
    /// while the library cannot run it: it declares no ports, and the user can
    /// still select, move and delete it.
    declaration: Declaration<'a>,
    /// What the last run said about this node, looked up once when the node
    /// resolves rather than at every status, memory or port read.
    run: Option<&'a NodeRunState>,
    /// Whether the pointer is over this node's body — [`Hover::Rest`] until a
    /// caller that holds a `Ui` sets it through [`Self::with_hover`].
    ///
    /// Two things hang off it, the const editors' hover-revealed chips
    /// ([`Self::const_editor_theme`]) and whether the port rows build tooltips at all
    /// ([`Self::tooltips`]). One field rather than a `&ConstEditorTheme` and a
    /// flag threaded down beside each other, where no signature would say the
    /// two are the same fact.
    ///
    /// Not resolved in [`Self::resolve`] because that takes no `Ui` — and
    /// must not, since it is what the whole frame reads nodes through. The
    /// record pass sets it once per node body instead.
    hover: Hover,
}

impl<'a> NodeCtx<'a> {
    /// Resolve the placed `node_id` against the document.
    ///
    /// # Panics
    ///
    /// If the document holds no `node_id`: callers resolve placements, and the
    /// document holds a placement exactly for every graph node and stub.
    pub(super) fn resolve(graph_ctx: GraphCtx<'a>, node_id: NodeId, pos: Vec2) -> Self {
        let library = graph_ctx.library();
        let (node, declaration) = if let Some(node) = graph_ctx.body().find(node_id) {
            (node, Declaration::Func(node.func(library)))
        } else {
            let record = graph_ctx
                .document()
                .stubs
                .get(node_id)
                .expect("every placement names a graph node or a stub");
            let reason = library
                .func_of(record.node())
                .expect_err("a stub does not resolve");
            (record.node(), Declaration::Stub(reason))
        };
        Self {
            graph_ctx,
            node_id,
            pos,
            node,
            declaration,
            run: graph_ctx.run_state().node(node_id),
            hover: Hover::Rest,
        }
    }

    /// This node with the pointer-over question answered, resolved once by the
    /// record pass for everything below the node body that asks it.
    ///
    /// Takes the answer rather than a `Ui`: the widget id it comes off is
    /// `gui::pane::graph_ui::node_ui`'s (`wid::hover`), and this type sits below
    /// that module.
    pub(crate) const fn with_hover(self, hover: Hover) -> Self {
        Self { hover, ..self }
    }

    /// Whether the port rows build their hover tooltips: their text is
    /// composed per port per frame, and no port can be showing one while the
    /// pointer is elsewhere, so only the node under it pays.
    pub(crate) const fn tooltips(self) -> bool {
        matches!(self.hover, Hover::Over)
    }

    /// The const-editor styling this node's value cells paint with. Pointer
    /// over the node surfaces the (otherwise invisible) chips at half
    /// strength — the edit affordance appears exactly when the pointer is in
    /// the neighborhood, and geometry never changes.
    pub(crate) const fn const_editor_theme(self) -> &'a ConstEditorTheme {
        let theme = self.graph_ctx.theme();
        match self.hover {
            Hover::Over => &theme.const_editor_revealed,
            Hover::Rest => &theme.const_editor,
        }
    }

    /// The palette and metrics this node paints from.
    pub(crate) const fn theme(self) -> &'a Theme {
        self.graph_ctx.theme()
    }

    /// The user's name for the node, empty when it carries none.
    pub(crate) fn name(self) -> &'a str {
        &self.node.name
    }

    /// Human-readable type identity: the func's name, or for a stub whether its
    /// func is gone or changed its ports. Shown by the inspector pane.
    pub(crate) fn kind_label(self) -> &'a str {
        match self.declaration {
            Declaration::Func(func) => func.name(),
            Declaration::Stub(Unresolved::ChangedPorts) => CHANGED_FUNC_LABEL,
            Declaration::Stub(Unresolved::MissingFunc) => MISSING_FUNC_LABEL,
        }
    }

    /// The func's description, empty for a stub or a func that declares none.
    /// Shown by the inspector pane and the new-node palette tooltip.
    pub(crate) fn description(self) -> &'a str {
        self.func().and_then(Func::description).unwrap_or_default()
    }

    /// A stub: a node the library cannot resolve, kept beside the graph.
    /// Rendered as a portless error card the user can still select, move and
    /// delete — never silently dropped.
    pub(crate) const fn stub(self) -> bool {
        matches!(self.declaration, Declaration::Stub(_))
    }

    /// Excluded from execution (`Node::disabled`). Sink headers expose the
    /// toggle; the body paints any authored disabled node dimmed.
    pub(crate) const fn disabled(self) -> bool {
        self.node.disabled
    }

    /// Where this node's output is cached. The header's two storage chips
    /// toggle its RAM and disk bits.
    pub(crate) const fn cache(self) -> CacheMode {
        self.node.cache
    }

    /// Sink node — no outputs feed downstream, so a run has to reach it
    /// explicitly.
    ///
    /// Read off the declaration, like every other interface fact: the flag
    /// is the func's own, and a compile copies it verbatim rather than
    /// deriving anything. Answering from the authoring side is also what
    /// lets the affordances it gates — the disable toggle, the subscription
    /// pin — render before the first compile.
    pub(crate) fn sink(self) -> bool {
        self.func().is_some_and(Func::sink)
    }

    /// The node holds work that recomputes every run. An impure node has no
    /// content digest, so no cache mode is ever honored; the header paints
    /// the `~` marker off this. Declared, like [`Self::sink`].
    pub(crate) fn impure(self) -> bool {
        self.func().is_some_and(Func::impure)
    }

    /// A preview node: its body shows the value wired into it instead of the
    /// usual output ports and memory readout. A stub is never one, so a
    /// document whose library lost the func shows an ordinary stub card rather
    /// than an empty preview.
    pub(crate) fn preview(self) -> bool {
        !self.stub() && self.node.func_id().is_some_and(preview_sink::is_func)
    }

    /// Whether the node has an output a cache can key: the one rule behind
    /// the header's RAM/disk storage chips and its eviction chip. An impure
    /// func has no content digest to key a cache on, and a func with no
    /// outputs has nothing to store — a stub for both reasons at once.
    pub(crate) fn cacheable(self) -> bool {
        self.func()
            .is_some_and(|f| !f.outputs().is_empty() && !f.impure())
    }

    /// Whether Darkroom exposes the disable toggle. Limiting it to runnable
    /// sinks keeps disabled nodes directly runnable with their upstream cone
    /// intact.
    pub(crate) fn can_disable(self) -> bool {
        self.sink()
    }

    /// Whether this node can seed a "run to this node" — drives the header
    /// play chip, the context-menu item, and the port menu's "Add preview".
    ///
    /// Everything but a stub qualifies: the stub is not in the graph, so it
    /// has no compiled work, while a *disabled* node still runs, because a targeted
    /// run overrides that flag for the run. Deliberately an authoring-side
    /// fact, not a lookup in a compiled program — the palette and the header
    /// record every frame, including before the first compile, so an
    /// affordance can't wait on one.
    pub(crate) const fn runnable(self) -> bool {
        !self.stub()
    }

    /// Outcome of the last graph run, with its detail. Drives the node's
    /// status frame, the footer's status, the inspector's status and
    /// error lines, and the highlight on the ports the run could not feed.
    pub(crate) fn exec_status(self) -> &'a ExecStatus {
        self.run.map_or(&ExecStatus::None, NodeRunState::status)
    }

    /// RAM this node's cached output currently holds (system vs GPU).
    /// Non-zero only for nodes that retain a value; drives the node body's
    /// memory readout, hidden when zero.
    pub(crate) fn ram(self) -> RamUsage {
        self.run.map(NodeRunState::ram).unwrap_or_default()
    }

    /// This node's input ports, in declaration order. Empty for a stub, which
    /// declares none.
    pub(crate) fn inputs(self) -> impl ExactSizeIterator<Item = InputCtx<'a>> {
        let declared = self.func().map_or(&[][..], Func::inputs);
        declared
            .iter()
            .enumerate()
            .map(move |(port_idx, input)| InputCtx::new(self, port_idx, input))
    }

    /// The func this node instantiates, `None` for a stub.
    pub(super) const fn func(self) -> Option<&'a Func> {
        match self.declaration {
            Declaration::Func(func) => Some(func),
            Declaration::Stub(_) => None,
        }
    }

    /// One input port by index.
    pub(crate) fn input(self, port_idx: usize) -> Option<InputCtx<'a>> {
        let declared = self.func()?.inputs().get(port_idx)?;
        Some(InputCtx::new(self, port_idx, declared))
    }

    /// This node's output ports, in declaration order.
    pub(crate) fn outputs(self) -> impl ExactSizeIterator<Item = OutputCtx<'a>> {
        let declared = self.func().map_or(&[][..], Func::outputs);
        declared
            .iter()
            .enumerate()
            .map(move |(port_idx, output)| OutputCtx::new(self, port_idx, output))
    }

    /// This node's event (emitter) ports. Events carry no data type — they
    /// are pure triggers — so the declaration is all the UI needs, and the
    /// output column lists them under the data outputs.
    pub(crate) fn events(self) -> &'a [FuncEvent] {
        self.func().map_or(&[][..], Func::events)
    }

    /// How many ports this node declares on `kind`'s side.
    pub(crate) fn port_count(self, kind: PortKind) -> usize {
        match kind {
            PortKind::Input => self.func().map_or(0, |f| f.inputs().len()),
            PortKind::Output => self.func().map_or(0, |f| f.outputs().len()),
        }
    }

    /// Every `PortRef` on the given side, in port order. Single source for
    /// the "iterate a node's ports by kind" loop `CanvasGeometry::rebuild`
    /// and the wire scans all need, so scan order and paint order
    /// can't drift apart.
    pub(crate) fn ports(self, kind: PortKind) -> impl Iterator<Item = PortRef> {
        let node_id = self.node_id;
        (0..self.port_count(kind)).map(move |port_idx| PortRef {
            node_id,
            kind,
            port_idx,
        })
    }

    /// Every `EventPort`, in declaration order — the emitter-glyph
    /// counterpart of [`Self::ports`], shared by `CanvasGeometry::rebuild`
    /// and the subscription-wire scans for the same reason.
    pub(crate) fn event_ports(self) -> impl Iterator<Item = EventPort> {
        let node_id = self.node_id;
        (0..self.events().len()).map(move |event_idx| EventPort::new(node_id, event_idx))
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
