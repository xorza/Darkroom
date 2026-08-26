//! One node of the graph, with its declaration resolved.

use glam::Vec2;
use scenarium::{CacheMode, Func, FuncEvent, Node, NodeId, NodeKind, RamUsage};

use crate::core::document::{PortKind, PortRef};
use crate::core::preview;
use crate::gui::EventRef;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::input_ctx::InputCtx;
use crate::gui::graph_ctx::output_ctx::OutputCtx;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;
use crate::gui::theme::const_value_editor_theme::ConstValueEditorTheme;

/// The `kind_label` a node reports when the library holds no func for it —
/// see [`NodeCtx::missing`].
const MISSING_FUNC_LABEL: &str = "missing func";

/// One node of the graph: its authoring record, its position in the paint
/// stack, and the declaration the library resolves for it.
///
/// The leaf of the context chain, and the one level where the *item* is the
/// level — every function below a node body is about that node, so passing it
/// beside the context would be the same argument twice.
///
/// **Available for the whole frame**, unlike the passes above it: a
/// [`GraphCtx`] is all it takes to resolve one, so the hit sweep at frame top,
/// the prepass gestures, and the record all read nodes through this same type.
/// The facts that only a *record* has settled — the effective selection, the
/// open inspectors, the cull region — stay on
/// [`DrawCtx`](crate::gui::pane::graph::ctx::DrawCtx), which travels beside
/// this rather than inside it, so a reader cannot reach for one in a phase
/// where it does not exist yet.
///
/// `Copy`, and the three borrows are taken once at [`Self::resolve`] — so a
/// pass that threads one down a widget tree pays a single library lookup for
/// the node, not one per field it reads.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NodeCtx<'a> {
    pub(crate) graph_ctx: GraphCtx<'a>,
    pub(crate) id: NodeId,
    /// Where the node's body sits, from the view's placements. A field rather
    /// than an accessor: the paint stack is walked by position, and every
    /// geometry reader wants it.
    pub(crate) pos: Vec2,
    node: &'a Node,
    /// The declaration scenarium resolves — a library func or a special
    /// node's hardcoded spec.
    ///
    /// `None` is **library drift**, not a failure to look something up: a
    /// document loads against whatever library is present, and
    /// `Graph::validate` (the structural walk the load path runs) tolerates a
    /// func id the current library lacks so the authored wiring survives the
    /// library coming back. It reads as a portless
    /// [`missing`](Self::missing) stub the user can still select and delete,
    /// and a run refuses it loudly — `validate_with` rejects `MissingFunc` at
    /// compile, which the status bar shows.
    ///
    /// Every accessor below degrades it to "declares nothing" rather than
    /// guessing; that is the stub, and it is what a new accessor here must
    /// keep doing.
    func: Option<&'a Func>,
    /// The input ports the last run could not feed, by index — one lookup
    /// per node rather than one per port.
    missing_inputs: &'a [usize],
    /// Whether the pointer is over this node's body — `false` until a caller
    /// that holds a `Ui` sets it through [`Self::with_hover`].
    ///
    /// Two things hang off it, the const editors' hover-revealed chips
    /// ([`Self::sve`]) and whether the port rows build tooltips at all
    /// ([`Self::tips`]). One field rather than a `&ConstValueEditorTheme` and a
    /// `bool` threaded down beside each other, where no signature would say the
    /// two are the same fact.
    ///
    /// Not resolved in [`Self::resolve`] because that takes no `Ui` — and
    /// must not, since it is what the whole frame reads nodes through. The
    /// record pass sets it once per node body instead.
    hovered: bool,
}

impl<'a> NodeCtx<'a> {
    /// Resolve `node_id` against the graph, or `None` when the graph does not
    /// hold it — a placement left behind by a delete.
    pub(super) fn resolve(graph_ctx: GraphCtx<'a>, node_id: NodeId, pos: Vec2) -> Option<Self> {
        let node = graph_ctx.body().find(node_id)?;
        // `None` has exactly one meaning: a `NodeKind::Func` whose id the
        // library no longer holds. A special node's declaration is hardcoded,
        // so `Graph::node_func` resolves it unconditionally.
        let func = graph_ctx.body().node_func(node, graph_ctx.library());
        Some(Self {
            graph_ctx,
            id: node_id,
            pos,
            node,
            func,
            missing_inputs: graph_ctx.run_state().missing_inputs(node_id),
            hovered: false,
        })
    }

    /// This node with the pointer-over question answered, resolved once by the
    /// record pass for everything below the node body that asks it.
    ///
    /// Takes the answer rather than a `Ui`: the widget id it comes off is
    /// `gui::pane::graph::node`'s (`wid::hovered`), and this type sits below
    /// that module.
    pub(crate) fn with_hover(self, hovered: bool) -> Self {
        Self { hovered, ..self }
    }

    /// Whether the port rows build their hover tooltips: their text is
    /// composed per port per frame, and no port can be showing one while the
    /// pointer is elsewhere, so only the node under it pays.
    pub(crate) fn tips(self) -> bool {
        self.hovered
    }

    /// The const-editor styling this node's value cells paint with. Pointer
    /// over the node surfaces the (otherwise invisible) chips at half
    /// strength — the edit affordance appears exactly when the pointer is in
    /// the neighborhood, and geometry never changes.
    pub(crate) fn sve(self) -> &'a ConstValueEditorTheme {
        let theme = self.graph_ctx.theme();
        if self.hovered {
            &theme.const_value_editor_revealed
        } else {
            &theme.const_value_editor
        }
    }

    /// The palette and metrics this node paints from.
    pub(crate) fn theme(self) -> &'a Theme {
        self.graph_ctx.theme()
    }

    /// The user's name for the node, empty when it carries none.
    pub(crate) fn name(self) -> &'a str {
        &self.node.name
    }

    /// Human-readable type identity: the func's name, or "missing func" for a
    /// stub. Shown by the inspection panel.
    pub(crate) fn kind_label(self) -> &'a str {
        self.func.map_or(MISSING_FUNC_LABEL, |f| f.name.as_str())
    }

    /// The func's description, empty for a stub or a func that declares none.
    /// Shown by the inspection panel and the new-node palette tooltip.
    pub(crate) fn description(self) -> &'a str {
        self.func
            .and_then(|f| f.description.as_deref())
            .unwrap_or_default()
    }

    /// The node's func is absent from the library (e.g. a document saved
    /// against an older library), so its interface can't be resolved.
    /// Rendered as a portless error stub the user can still select and
    /// delete — never silently dropped.
    pub(crate) fn missing(self) -> bool {
        self.func.is_none()
    }

    /// Excluded from execution (`Node::disabled`). Sink headers expose the
    /// toggle; the body paints any authored disabled node dimmed.
    pub(crate) fn disabled(self) -> bool {
        self.node.disabled
    }

    /// Where this node's output is cached. The header's two storage chips
    /// toggle its RAM and disk bits.
    pub(crate) fn cache(self) -> CacheMode {
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
        self.func.is_some_and(|f| f.sink)
    }

    /// The node holds work that recomputes every run. An impure node has no
    /// content digest, so no cache mode is ever honored; the header paints
    /// the `~` marker off this. Declared, like [`Self::sink`].
    pub(crate) fn impure(self) -> bool {
        self.func.is_some_and(|f| f.impure())
    }

    /// A preview node: its body shows the value wired into it instead of the
    /// usual output ports and memory readout. Resolved from the func id, so a
    /// document whose library lost the func degrades to an ordinary missing
    /// stub rather than an empty card.
    pub(crate) fn preview(self) -> bool {
        self.func.is_some()
            && matches!(self.node.kind, NodeKind::Func(func_id) if preview::is_preview(func_id))
    }

    /// Whether the header offers the RAM/disk storage chips. An impure func
    /// has no content digest to key a cache on, and a func that declares
    /// itself uncacheable or exposes no outputs has nothing to store — a
    /// `missing` stub for both reasons at once.
    pub(crate) fn cache_controls(self) -> bool {
        self.func
            .is_some_and(|f| !f.uncacheable && !f.outputs.is_empty() && !f.impure())
    }

    /// Whether the header offers runtime cache eviction — it needs a
    /// reproducible output, which an impure or portless node has not.
    pub(crate) fn can_evict_cache(self) -> bool {
        self.func
            .is_some_and(|f| !f.outputs.is_empty() && !f.impure())
    }

    /// Whether Darkroom exposes the disable toggle. Limiting it to runnable
    /// sinks keeps disabled nodes directly runnable with their upstream cone
    /// intact.
    pub(crate) fn can_disable(self) -> bool {
        self.sink() && !self.missing()
    }

    /// Whether this node can seed a "run to this node" — drives the header
    /// play chip, the context-menu item, and the port menu's "Add preview".
    ///
    /// Everything but a `missing` stub qualifies: the stub resolves to no
    /// compiled work, while a *disabled* node still runs, because a targeted
    /// run overrides that flag for the run. Deliberately an authoring-side
    /// fact, not a lookup in a compiled program — the palette and the header
    /// record every frame, including before the first compile, so an
    /// affordance can't wait on one.
    pub(crate) fn runnable(self) -> bool {
        !self.missing()
    }

    /// Outcome of the last graph run. Drives the node's status-glow shadow
    /// and (for `Executed`) the header time label.
    pub(crate) fn exec_status(self) -> ExecStatus {
        self.graph_ctx.run_state().status(self.id)
    }

    /// RAM this node's cached output currently holds (system vs GPU).
    /// Non-zero only for nodes that retain a value; drives the node body's
    /// memory readout, hidden when zero.
    pub(crate) fn ram(self) -> RamUsage {
        self.graph_ctx.run_state().ram(self.id)
    }

    /// This node's input ports, in declaration order. Empty for a stub, which
    /// declares nothing.
    pub(crate) fn inputs(self) -> impl ExactSizeIterator<Item = InputCtx<'a>> {
        let declared = self.func.map_or(&[][..], |f| f.inputs.as_slice());
        declared
            .iter()
            .enumerate()
            .map(move |(port_idx, input)| InputCtx::new(self, port_idx, input))
    }

    /// One input port by index.
    pub(crate) fn input(self, port_idx: usize) -> Option<InputCtx<'a>> {
        let declared = self.func?.inputs.get(port_idx)?;
        Some(InputCtx::new(self, port_idx, declared))
    }

    /// This node's output ports, in declaration order.
    pub(crate) fn outputs(self) -> impl ExactSizeIterator<Item = OutputCtx<'a>> {
        let declared = self.func.map_or(&[][..], |f| f.outputs.as_slice());
        declared
            .iter()
            .enumerate()
            .map(move |(port_idx, output)| OutputCtx::new(self, port_idx, output))
    }

    /// One output port by index.
    pub(crate) fn output(self, port_idx: usize) -> Option<OutputCtx<'a>> {
        let declared = self.func?.outputs.get(port_idx)?;
        Some(OutputCtx::new(self, port_idx, declared))
    }

    /// This node's event (emitter) ports. Events carry no data type — they
    /// are pure triggers — so the declaration is all the UI needs, and the
    /// output column lists them under the data outputs.
    pub(crate) fn events(self) -> &'a [FuncEvent] {
        self.func.map_or(&[][..], |f| f.events.as_slice())
    }

    /// How many ports this node declares on `kind`'s side.
    pub(crate) fn port_count(self, kind: PortKind) -> usize {
        match kind {
            PortKind::Input => self.func.map_or(0, |f| f.inputs.len()),
            PortKind::Output => self.func.map_or(0, |f| f.outputs.len()),
        }
    }

    /// Every `PortRef` on the given side, in port order. Single source for
    /// the "iterate a node's ports by kind" loop `CanvasGeometry::rebuild`
    /// and the connection scans all need, so scan order and paint order
    /// can't drift apart.
    pub(crate) fn ports(self, kind: PortKind) -> impl Iterator<Item = PortRef> {
        let node_id = self.id;
        (0..self.port_count(kind)).map(move |port_idx| PortRef {
            node_id,
            kind,
            port_idx,
        })
    }

    /// Every `EventRef`, in declaration order — the emitter-glyph
    /// counterpart of [`Self::ports`], shared by `CanvasGeometry::rebuild`
    /// and the subscription-wire scans for the same reason.
    pub(crate) fn event_refs(self) -> impl Iterator<Item = EventRef> {
        let node_id = self.id;
        (0..self.events().len()).map(move |event_idx| EventRef { node_id, event_idx })
    }

    /// The port indices the last run could not feed — its own verdict, so a
    /// port wired to a disabled or itself-unfed producer counts too, not just
    /// an unbound one.
    pub(super) fn missing_inputs(self) -> &'a [usize] {
        self.missing_inputs
    }
}
