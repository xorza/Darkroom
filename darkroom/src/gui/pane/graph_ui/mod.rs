//! The graph pane: the pan/zoom canvas, everything recorded on it, and the
//! toolbar pinned to its corner.
//!
//! Split by what each part does with a frame rather than by what it draws:
//! [`frame`] is what a pass reads, [`gesture`] is what turns input into
//! intents, [`draw`] is what the record pass draws between the node bodies,
//! and [`node_ui`] is the body itself. [`canvas`] holds the two nested canvases'
//! ids and the coordinate space between them — the one thing every one of
//! those needs.
//!
//! This file is [`GraphUi`] and nothing else: the state a graph pane keeps
//! across frames, the two phases that drive it ([`GraphUi::prepass`] and
//! [`GraphUi::draw`]), and the cache sweep ([`GraphUi::retain_nodes`]) that
//! runs outside them.

pub(crate) mod canvas;
pub(crate) mod canvas_bg;
pub(crate) mod canvas_ctx;
pub(crate) mod draw;
pub(crate) mod draw_ctx;
pub(crate) mod frame;
pub(crate) mod gesture;
pub(crate) mod node_ui;
pub(crate) mod prepass_ctx;
pub(crate) mod selection;
pub(crate) mod toolbar;
pub(crate) mod wid;

use palantir::{Chrome, Configure, Key, Panel, Sense, Sizing, TranslateScale, Ui};

use crate::core::document::Document;
use crate::gui::frame::relayout::Relayout;
use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::canvas_bg::CanvasBg;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::draw::wire::{WireEmphasis, WirePass, WireSet};
use crate::gui::pane::graph_ui::draw_ctx::{DrawCtx, PaintFacts};
use crate::gui::pane::graph_ui::frame::canvas_geometry::CanvasGeometry;
use crate::gui::pane::graph_ui::frame::cull_region::CullRegion;
use crate::gui::pane::graph_ui::frame::tie_layout::{Span, TieLayout};
use crate::gui::pane::graph_ui::gesture::breaker_ui::BreakerUi;
use crate::gui::pane::graph_ui::gesture::group_drag::GroupDrag;
use crate::gui::pane::graph_ui::gesture::new_node_ui::NewNodeUi;
use crate::gui::pane::graph_ui::gesture::node_menu_ui::NodeMenuUi;
use crate::gui::pane::graph_ui::gesture::pan_zoom::Framing;
use crate::gui::pane::graph_ui::gesture::pan_zoom::camera_gesture::CameraGesture;
use crate::gui::pane::graph_ui::gesture::preview_drag;
use crate::gui::pane::graph_ui::gesture::selection_ui::SelectionUi;
use crate::gui::pane::graph_ui::gesture::shortcuts;
use crate::gui::pane::graph_ui::gesture::subscription_ui::SubscriptionUi;
use crate::gui::pane::graph_ui::gesture::tie_ui::TieUi;
use crate::gui::pane::graph_ui::gesture::tie_ui::tie_menu_ui::TieMenuUi;
use crate::gui::pane::graph_ui::gesture::tie_ui::tie_widget::{TiePass, TieWidgets, TiesRecorded};
use crate::gui::pane::graph_ui::gesture::wire_ui::{WireEnd, WireUi};
use crate::gui::pane::graph_ui::node_ui::{NodeDrawOutcome, NodePass, NodeUi};
use crate::gui::pane::graph_ui::prepass_ctx::{
    CanvasGesture, CanvasInput, Escape, PrepassCtx, SecondaryPress,
};
use crate::gui::pane::graph_ui::selection::Selection;

/// The graph pane's UI state: the cross-frame geometry, the `NodeUi` that
/// renders graph nodes, the inspection panels, and the in-flight gesture
/// controllers.
///
/// The document has one graph tab, so one pane shows it and one instance
/// drives it: there is one pointer, so one drag, one rubber band, one open
/// popup. The canvas widget ids are fixed for the same reason.
///
/// Nothing here builds the [`GraphCtx`] it reads: `MainWindow` composes one
/// per frame phase and threads it down, so this type never needs to name a
/// theme, a library or a run state — the context answers for all three — and
/// the "is the graph pane up" question is settled once at the tab dispatch
/// rather than re-derived by every pass.
///
/// The frame splits accordingly: [`Self::prepass`] turns this frame's input
/// into intents before the record, and [`Self::draw`] records the pane from
/// the dock's content closure.
///
/// **Bare-canvas gesture arbitration.** [`CanvasGesture::classify`] reads the
/// outer canvas's response ([`canvas::outer_wid`]) + modifiers and
/// resolves which gesture latches this frame into a single [`CanvasGesture`].
/// `prepass` resolves it once per frame and parks it in [`Self::input`];
/// each sub-controller is handed that classification and consumes only its
/// own variant, so there's no hand-kept disjointness across files — the
/// precedence lives in one match. Wheel/pinch zoom isn't a latch gesture (it
/// coexists), so it stays inside `CameraGesture::prepass` regardless of the
/// classification.
///
/// Node panels and port circles live in the *inner* canvas and hit-test
/// first, so a gesture only reaches the bare canvas (and this
/// classification) when it missed every node/port.
#[derive(Default, Debug)]
pub(crate) struct GraphUi {
    /// The *record* frame this canvas last ran [`Self::prepass`] on, or `None`
    /// before its first ever one.
    ///
    /// The prepass runs only while a pane is showing the graph, so a *gap*
    /// here means the canvas was away — which is how it notices its own
    /// reappearance without anything having to run on its behalf while it was
    /// gone. A stamp rather than a cached `bool` copy of
    /// [`Document::shows_graph`], so there is no second spelling of visibility
    /// to keep in step with the first.
    ///
    /// [`Ui::frame_id`] and not [`Ui::render_frame_id`]: the latter counts
    /// painted frames too, and a paint-only frame runs no record pass at all —
    /// so an idle caret blink (the new-node palette's search field is enough)
    /// would open a gap here while the canvas never went anywhere, and the next
    /// frame would drop every in-flight gesture.
    last_prepass_frame: Option<u64>,
    bg: CanvasBg,
    /// Port centers, node sizes and world rects, cached across frames and
    /// rebuilt in [`Self::prepass`]. Outlives the scene on purpose — a culled
    /// node's ports still resolve off it — so it is swept by
    /// [`Self::retain_nodes`] rather than by absence from a projection.
    ///
    /// Private: every production reader reaches it through a [`CanvasCtx`],
    /// which is the only thing that pairs it with the gesture it was settled
    /// beside. Tests read it through `internals`' `GraphUi::geometry`.
    geometry: CanvasGeometry,
    /// This frame's wire ties: each band and each tied wire's pieces, rebuilt
    /// at the top of [`Self::record_canvas`] into the buffers it keeps.
    ties: TieLayout,
    /// What this frame's input means on the canvas — the bare-canvas gesture
    /// that latched and what Esc does — resolved once in [`Self::prepass`]
    /// and read back by [`Self::draw`].
    input: CanvasInput,
    /// Whether the right press now held, or released this frame, already ended a floating wire,
    /// whose release then opens no palette: palantir reports the click on the release, frames
    /// after the press that ended the wire.
    secondary: SecondaryPress,
    // The in-flight gesture controllers, each dropped by
    // [`Self::reset_gestures`] on a tab switch. Flat rather than grouped
    // behind one field: replacing a group wholesale threw away buffers
    // several of them deliberately grow (the rubber band's swept set, the
    // breaker's point and broken-target vectors, the palette's folded search),
    // and each controller knows which of its own state is a buffer and which
    // is the gesture.
    node_ui: NodeUi,
    /// The one group drag a pointer holds, latched off a node's, a tie's or a preview spawn's
    /// press.
    group_drag: GroupDrag,
    breaker_ui: BreakerUi,
    wire_ui: WireUi,
    subscription_ui: SubscriptionUi,
    new_node_ui: NewNodeUi,
    node_menu: NodeMenuUi,
    selection_ui: SelectionUi,
    tie_ui: TieUi,
    tie_widgets: TieWidgets,
    tie_menu: TieMenuUi,
    /// The camera's input in flight: a pan drag's latch, and the run of
    /// wheel or pinch input. Input bookkeeping, not viewport state.
    camera: CameraGesture,
    /// A framing a surface outside the canvas asked for — the status bar's link, a menu —
    /// raised as a camera move in the next prepass, which has the settled geometry it needs.
    framing: Option<Framing>,
}

impl GraphUi {
    /// Raise `framing` as a camera move in the next prepass.
    pub(crate) const fn request_framing(&mut self, framing: Framing) {
        self.framing = Some(framing);
    }

    /// Evict this canvas's `NodeId`-keyed cache, the cross-frame geometry, down to the nodes the
    /// document still holds.
    ///
    /// It deliberately outlives the scene, so absence from it is no grounds on its own; see
    /// [`CanvasGeometry::retain_nodes`].
    pub(crate) fn retain_nodes(&mut self, document: &Document) {
        self.geometry
            .retain_nodes(|node_id| document.holds_node(node_id));
    }

    /// Drop every in-flight gesture, keeping the buffers the controllers grow.
    ///
    /// Gestures only.
    ///
    /// Destructured rather than assigned wholesale so the compiler makes the
    /// call: a field added to [`GraphUi`] does not compile until it is either
    /// dropped here or explicitly named as surviving. The `_`-bound arms below
    /// are the survivors — cross-frame caches and per-frame facts that the
    /// next frame overwrites anyway.
    fn reset_gestures(&mut self) {
        let Self {
            node_ui,
            group_drag,
            breaker_ui,
            wire_ui,
            subscription_ui,
            new_node_ui,
            node_menu,
            selection_ui,
            tie_ui,
            tie_widgets,
            tie_menu,
            camera,
            secondary,
            // Survivors: caches that outlive the scene on purpose,
            // and the frame-local facts `prepass` rewrites before anything
            // reads them.
            bg: _,
            geometry: _,
            ties: _,
            last_prepass_frame: _,
            input: _,
            // A request, not a gesture: it waits for the canvas to show.
            framing: _,
        } = self;
        node_ui.reset();
        breaker_ui.reset();
        wire_ui.reset();
        group_drag.reset();
        subscription_ui.reset();
        new_node_ui.reset();
        node_menu.reset();
        selection_ui.reset();
        tie_ui.reset();
        tie_widgets.reset();
        tie_menu.reset();
        camera.reset();
        *secondary = SecondaryPress::Free;
    }

    /// Whether a gesture, a menu or the palette is in flight — what Esc
    /// cancels before it would deselect.
    ///
    /// Destructured like [`Self::reset_gestures`], so a new controller does not
    /// compile until it is asked here too.
    const fn in_flight(&self) -> bool {
        let Self {
            node_ui: _,
            group_drag,
            breaker_ui,
            wire_ui,
            subscription_ui,
            new_node_ui,
            node_menu,
            selection_ui,
            tie_ui,
            tie_widgets,
            tie_menu,
            camera,
            bg: _,
            geometry: _,
            ties: _,
            last_prepass_frame: _,
            input: _,
            secondary: _,
            framing: _,
        } = self;
        group_drag.in_flight()
            || breaker_ui.in_flight()
            || wire_ui.in_flight()
            || subscription_ui.in_flight()
            || new_node_ui.in_flight()
            || node_menu.in_flight()
            || selection_ui.in_flight()
            || tie_ui.in_flight()
            || tie_widgets.in_flight()
            || tie_menu.in_flight()
            || camera.in_flight()
    }

    /// Take note of whether the pane is showing this canvas, and report
    /// whether that *changed* since last frame.
    ///
    /// Crossing the edge drops all in-flight gesture state: a drag left
    /// latched while the canvas was away would otherwise resume when it
    /// comes back. The inspection panels and the cross-frame caches survive —
    /// notably
    /// [`CanvasGeometry`]'s port-offset table, so wires still anchor on
    /// the first frame after a switch.
    ///
    /// Whether this prepass is the canvas's first after being away — the
    /// edge that owes a relayout, since a canvas that has never recorded has
    /// no cached geometry to draw its first frame from, and a dock op raises
    /// no edit fact of its own.
    ///
    /// Reads the frame stamp rather than a visibility flag, so nothing has to
    /// run while the canvas is off screen for it to notice coming back. Two
    /// cases both mean *still here* and must not read as an appearance: the
    /// previous frame (the ordinary steady state) and *this* frame — a frame
    /// that owes a relayout records twice, and both passes read the same
    /// [`Ui::frame_id`]. Only a gap wider than that is an absence.
    fn appearing(&mut self, ui: &Ui) -> bool {
        let frame = ui.frame_id();
        let appearing = self
            .last_prepass_frame
            .is_none_or(|last| last.saturating_add(1) < frame);
        self.last_prepass_frame = Some(frame);
        appearing
    }

    pub(crate) fn prepass(
        &mut self,
        ui: &mut Ui,
        graph_ctx: GraphCtx<'_>,
        out: &mut Requests,
    ) -> Relayout {
        debug_assert!(
            graph_ctx.is_visible(),
            "the prepass is reached from an active Graph tab"
        );
        // First: a canvas back from being away drops the gesture state it left
        // latched, since a drag still held would otherwise resume under a
        // pointer that has long since moved on.
        let appearing = self.appearing(ui);
        if appearing {
            self.reset_gestures();
        }
        // Resolve the frame's bare-canvas gesture and park it for `draw` to
        // read back — the classification is one response poll, and both
        // phases must agree on the winner.
        //
        // One classification for the whole frame. A controller with nothing in
        // flight has nothing to cancel, so each applies this to its own
        // state slot unconditionally and lets its existing "no gesture"
        // path do the rest.
        // Esc cancels what is in flight, whatever the modifiers: Ctrl still held from a spawn
        // drag's chord must not swallow the cancel. Deselect, a command, is plain Esc alone. Both
        // read every frame, so each stays watched.
        let cancel = ui.key_pressed(Key::Escape);
        let plain = ui.shortcut_pressed(Key::Escape);
        let escape = if self.in_flight() && cancel {
            Escape::Cancels
        } else if plain {
            Escape::Deselects
        } else {
            Escape::Unpressed
        };
        let canvas = ui.response_for(canvas::outer_wid());
        if !canvas.right.held() && !canvas.right.released() {
            self.secondary = SecondaryPress::Free;
        }
        self.input = CanvasInput {
            gesture: CanvasGesture::classify(&canvas, ui.modifiers(), self.secondary),
            escape,
            dropped_wire: None,
        };
        let prepass = PrepassCtx::new(graph_ctx, self.input);
        self.camera.prepass(ui, prepass, out);
        let dragging = self.group_drag.prepass(ui, prepass, out);
        self.tie_widgets.prepass(ui, prepass, out);
        // One walk, filling the geometry caches off the per-node and per-port
        // responses.
        self.geometry.rebuild(ui, graph_ctx);
        // Everything below reads the settled geometry, so from here the canvas
        // has a context of its own.
        let ctx = CanvasCtx::new(prepass, &self.geometry);
        // Both port-drag claimants sit *after* the rebuild so they read this
        // frame's drag edges and centers, and `preview_drag_modifier` keeps
        // them disjoint: the preview spawn takes the output column under the
        // chord, the wire gesture takes it otherwise.
        if !dragging && let Some(start) = preview_drag::start(ui, ctx) {
            let gesture = self.group_drag.latch(start.grab, out);
            out.extend_graph_in(gesture, start.spawn);
        }
        match self.wire_ui.prepass(ui, ctx, out) {
            // The palette, which reads the input in the record, opens on the dropped wire.
            Some(WireEnd::DroppedOnCanvas(source)) => self.input.dropped_wire = Some(source),
            // The press that ended a floating wire is spent, its release too when it lands this
            // same frame: the palette sees no click.
            Some(WireEnd::Secondary) => {
                self.secondary = SecondaryPress::Spent;
                self.input.gesture =
                    CanvasGesture::classify(&canvas, ui.modifiers(), self.secondary);
            }
            None => {}
        }
        // Subscription wires (emitter → subscriber) latch/commit here, for
        // the same pre-record reasons as the wire gesture above; an
        // emitter glyph and a data port can't both latch (different widget-id
        // spaces).
        self.subscription_ui.prepass(ui, ctx, out);
        // Last, once: both wire gestures have settled their snap targets
        // above, and the flags this writes are document-unique, so the draw
        // reads finished geometry. Taking the table back `&mut` is what ends
        // `ctx` — nothing below may read one.
        self.wire_ui.bake_snap_hover(&mut self.geometry);
        self.subscription_ui.bake_snap_hover(&mut self.geometry);
        if let Some(framing) = self.framing.take() {
            out.extend_graph(framing.intent(ui, CanvasCtx::new(prepass, &self.geometry)));
        }
        // The keyboard half of the same phase. Last, so a chord reads the
        // document the pointer gestures above were raised against.
        shortcuts::emit(ui, CanvasCtx::new(prepass, &self.geometry), out);
        Relayout::needed_if(appearing)
    }

    /// Record one graph pane: its gestures' record-phase halves, the canvas
    /// itself, and the run/cancel toolbar over it. Called once per visible
    /// graph tab from the dock's content closure, so everything here is scoped
    /// to `graph_ctx` — the canvas widget ids included.
    ///
    /// Anything the pane asks for — graph edits, and the commands a chip or a
    /// menu pick means — lands on `out` in the order it was raised.
    pub(crate) fn draw(&mut self, ui: &mut Ui, graph_ctx: GraphCtx<'_>, out: &mut Requests) {
        debug_assert!(
            graph_ctx.is_visible(),
            "the record is reached from an active Graph tab"
        );
        // Pan/zoom was already folded into the document in `prepass`, and the
        // contexts built below read it straight off there, so the transform
        // sees the up-to-date viewport with nothing to re-sync. The gesture
        // and the Esc were resolved in `prepass` too, and every half below
        // composes the same context off them — so no two passes can disagree
        // about the frame they are drawing.
        self.resolve_gestures(ui, graph_ctx, out);
        // The toolbar overlays the canvas's top-left corner rather than
        // sitting beside it, so the two share a stack. It records *second*,
        // which is what puts it above the canvas in the hit-test: a click on a
        // chip never starts a pan. The stacking is the pane's own business —
        // the dock's content closure hands this method a plain `ui` and learns
        // nothing about the overlay.
        Panel::zstack()
            .id_salt("graph_overlay")
            .size(Sizing::FILL)
            .show(ui, |ui| {
                self.record_canvas(ui, graph_ctx, out);
                // Composed here rather than shared with the halves above and
                // below: both drive controllers and so hold all of `self`
                // mutably, while a context borrows the geometry shared. The
                // toolbar only reads, so it takes one built once
                // `record_canvas` has given the fields back.
                let ctx = CanvasCtx::new(PrepassCtx::new(graph_ctx, self.input), &self.geometry);
                toolbar::show(ui, ctx, out);
            });
    }

    /// The record pass's non-drawing half: each controller's record-phase
    /// `apply`, then the chip clicks that mean an
    /// [`AppCommand`](crate::gui::frame::app_command::AppCommand). Everything
    /// here reads last frame's responses and raises requests; nothing draws.
    fn resolve_gestures(&mut self, ui: &mut Ui, graph_ctx: GraphCtx<'_>, out: &mut Requests) {
        let ctx = CanvasCtx::new(PrepassCtx::new(graph_ctx, self.input), &self.geometry);
        // `CanvasGeometry` was already rebuilt in `prepass` against every
        // visible graph's scene — `App` rebuilds the scene *before* prepass
        // on the frame a tab becomes active, so prepass never sees a stale
        // graph, and the offset cache fills in port centers for nodes that
        // hadn't recorded yet. `ctx` carries that same table; no second
        // rebuild needed.
        self.selection_ui.apply(ui, ctx, out);
        self.breaker_ui.apply(ui, ctx, out);
        self.tie_ui.apply(ui, ctx, out);
        // A wire released over empty canvas (read in `prepass`) opens the new-node popup, and a
        // pick re-floats the wire.
        if let Some(source) = self.new_node_ui.apply(ui, ctx, out) {
            self.wire_ui.resume(source, graph_ctx);
        }
        // The menu records every frame whatever comes of it — its popup owns a
        // lifecycle that depends on it — and everything a pick means goes onto
        // `out`, so there is no precedence to settle here: a frame in which
        // both a menu pick and a chip click landed raises both.
        self.node_menu.apply(ui, ctx, out);
        self.tie_menu.apply(ui, ctx, &self.ties, out);
    }

    /// The record pass's drawing half: the outer (pan-capture) canvas, the
    /// dotted backdrop, and — under the inner canvas's pan/zoom transform —
    /// the wires, node bodies, inspection panels, and in-flight
    /// gesture previews.
    fn record_canvas(&mut self, ui: &mut Ui, graph_ctx: GraphCtx<'_>, out: &mut Requests) {
        self.geometry.reposition(graph_ctx.view());
        let geometry = &self.geometry;
        self.ties
            .rebuild(graph_ctx.view(), |wire| Span::of(geometry, wire));
        let ctx = CanvasCtx::new(PrepassCtx::new(graph_ctx, self.input), &self.geometry);
        let theme = ctx.theme();
        let viewport = graph_ctx.viewport();
        // Effective selection to paint: the live rubber-band preview while
        // a band is in flight, else the committed set.
        // The preview is kept out of the document, so a band in flight
        // changes what paints without recording an edit.
        let selected = self.selection_ui.preview().map_or_else(
            || Selection::Committed(graph_ctx.selected()),
            Selection::swept,
        );

        // What the node draw sees but cannot act on: the menu a right-click
        // opens and the drag a press starts, applied once the draw is over.
        let mut outcome = NodeDrawOutcome::default();
        // The tie a right-click landed on, whose menu opens once the draw is
        // over, as a node's does, and the drag a press on one started.
        let mut ties = TiesRecorded::default();

        // Outer canvas: covers the whole pane, paints the canvas
        // background, owns the input routing for empty-canvas
        //  Senses:
        // - `DRAG`: middle-button canvas pan (graph-editor
        //   convention; left-drag is the rubber-band selection). Pulled via
        //   `Ui::drag_delta_by(.., PointerButton::Middle)`, since the
        //   left-only `ResponseState::drag_delta` doesn't carry middle.
        // - `SCROLL`: mouse wheel / touchpad swipe = zoom-about-cursor.
        // - `PINCH`: touchpad pinch = zoom-about-cursor.
        // Node panels (descendants of the *inner* canvas, which
        // carries the pan/zoom transform) hit-test first; only bare
        // canvas falls through to the outer's senses.
        //
        // `.clip_rect()` pins the inner-canvas subtree's `paint_rect`s
        // to the outer rect even when the inner transform zooms them
        // way past the viewport. Without it, at high zoom a single
        // off-screen node panel's screen rect can dwarf the surface,
        // damage threshold sees ratio ≫ 1 and trips `Damage::Full`
        // every pan/zoom tick.
        Panel::canvas()
            .id(canvas::outer_wid())
            .size(Sizing::FILL)
            .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL | Sense::PINCH)
            .clip_rect()
            .chrome(Chrome::fill(theme.canvas.bg))
            .show(ui, |ui| {
                // Dotted backdrop in screen space, beneath the inner
                // (transformed) canvas — so it paints under everything.
                self.bg.draw(ui, theme, viewport);
                Panel::canvas()
                    .id(canvas::inner_wid())
                    .size(Sizing::FILL)
                    .transform(TranslateScale::new(viewport.pan, viewport.zoom))
                    .show(ui, |ui| {
                        let cull = CullRegion::from_size(
                            ui.response_for(canvas::outer_wid())
                                .layout_rect
                                .map(|rect| rect.size),
                            &viewport,
                        );
                        // Painted first so it sits beneath the
                        // wires and node bodies.
                        self.selection_ui.draw(ui, theme, viewport.zoom);
                        // One bundle for everything the canvas records, built
                        // out here rather than inside the probe scope so both
                        // passes read the same refs.
                        let draw_ctx = DrawCtx::new(ctx, PaintFacts { selected, cull });
                        {
                            let mut probe = self.breaker_ui.probe();
                            let mut tie_line = self.tie_ui.probe();
                            // One emphasis resolution for both wire families:
                            // any wire gesture — either drag controller, an
                            // active breaker scribble or a tie line — fades
                            // the standing set.
                            let set = if self.wire_ui.in_flight()
                                || self.subscription_ui.in_flight()
                                || probe.is_active()
                                || tie_line.is_active()
                            {
                                WireSet::Faded
                            } else {
                                WireSet::Standing
                            };
                            let emphasis = WireEmphasis::resolve(theme.canvas.bg, set);
                            // Both wire renderers share these inputs, so
                            // they're bundled once and reborrowed into each.
                            // Subscription wires sit under the node bodies
                            // like data wires (drawn before `draw_all`), and
                            // share the breaker probe so they're all
                            // cuttable — one passing behind an unrelated node
                            // goes under it rather than drawing on top.
                            let mut wires = WirePass {
                                draw_ctx,
                                probe: &mut probe,
                                tie_line: &mut tie_line,
                                lit: self.tie_menu.lit(),
                                emphasis: &emphasis,
                                ties: &self.ties,
                            };
                            wires.draw_data_wires(ui);
                            wires.draw_event_wires(ui);
                            // Over the wires they gather, under the node
                            // bodies, which cover a band as they cover wires
                            // and win a press over it.
                            ties = self.tie_widgets.record(
                                ui,
                                &mut TiePass {
                                    draw_ctx,
                                    ties: &self.ties,
                                    line: &mut tie_line,
                                    out,
                                },
                            );
                            // Node bodies paint back-to-front by each
                            // placement's depth (`GraphView::paint_order`), so
                            // a clicked node raises above its neighbours.
                            outcome = self.node_ui.draw_all(
                                ui,
                                &mut NodePass {
                                    draw_ctx,
                                    probe: &mut probe,
                                    out,
                                },
                            );
                        }
                        self.breaker_ui.draw(ui, theme, viewport.zoom);
                        self.tie_ui.draw(ui, theme, viewport.zoom);
                        self.wire_ui.draw_in_flight(ui, ctx);
                        self.subscription_ui.draw_in_flight(ui, ctx);
                    });
            });
        if let Some(node) = outcome.menu_opened {
            out.extend_graph(self.node_menu.open_on(ui, graph_ctx, node));
        } else if let Some(tie) = ties.menu_opened {
            out.extend_graph(self.tie_menu.open_on(ui, graph_ctx, tie));
        }
        // A body covers a band and wins a press over it, so at most one of them grabbed.
        if let Some(grab) = outcome.grab.take().or(ties.grab) {
            self.group_drag.latch(grab, out);
        }
    }
}

#[cfg(test)]
pub(crate) mod internals;

#[cfg(test)]
mod tests {

    use glam::UVec2;
    use palantir::Harness;

    use super::*;

    /// The prepass runs only while the pane shows the graph, so the frame
    /// stamp — not a visibility flag — is what tells the canvas it was away.
    ///
    /// Three cases must read as *still here*, and only a gap as an
    /// appearance. The repeat is the trap: a frame that owes a relayout
    /// records twice under one frame id, and a second call reading as an
    /// appearance would reset a drag mid-gesture.
    #[test]
    fn appearing_is_a_frame_gap_not_a_repeat_or_a_step() {
        let mut h = Harness::builder(UVec2::ONE).mono().build();
        let mut graph_ui = GraphUi::default();

        assert!(
            graph_ui.appearing(h.ui()),
            "a canvas that has never recorded is appearing"
        );
        assert!(
            !graph_ui.appearing(h.ui()),
            "a second pass of the same frame is the same appearance, not a new one"
        );

        h.frame(|_| {});
        assert!(
            !graph_ui.appearing(h.ui()),
            "the next consecutive frame is the steady state"
        );

        // Two frames the canvas sat out — the dock showed another tab.
        h.frame(|_| {});
        h.frame(|_| {});
        assert!(
            graph_ui.appearing(h.ui()),
            "a gap means it was away and is back"
        );
        assert!(
            !graph_ui.appearing(h.ui()),
            "and the reappearance is reported once, not per pass"
        );
    }
}
