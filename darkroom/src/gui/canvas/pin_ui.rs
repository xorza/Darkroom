//! A pinned output's wire, port-circle glyph, and drag gesture. The
//! widget's own look (the card, its header, and its image) lives in the
//! sibling [`crate::gui::canvas::pin_preview`] — this file draws the bezier
//! from the port to the widget's top-left corner, the port-circle glyph
//! peeking from under that corner (the same corner-peek trick as
//! [`crate::gui::node::header::subscription_pin`]), and owns the drag
//! gesture that positions it. A sibling of
//! [`crate::gui::canvas::connection_ui::ConnectionUI`], drawn at the canvas
//! level (like a wire) rather than nested in the node body, since a dragged
//! widget can end up anywhere on the canvas — not just overhanging its own
//! node. The card + glyph occupy their own slot in the shared paint stack
//! (`Scene::z_order`, interleaved with the node bodies by
//! [`crate::gui::node::NodeUI::draw_all`]), so a preview can sit above or
//! below any node and clicking or grabbing it raises it like a node body;
//! only the wire stays in the wires' own under-everything tier.
//!
//! Owns one gesture, unified across how it *starts*: Cmd+drag from an
//! output port's circle creates a fresh pin; a plain drag on an existing
//! preview widget repositions it (dragging one that's part of a
//! multi-selection moves the whole group — nodes and other pins alike —
//! together, via the same [`crate::core::edit::intent::types::Intent::MoveSelection`]
//! [`crate::gui::node::NodeUI`]'s node-body drag emits). Both commit
//! continuously — every frame the drag is held, not just on release —
//! exactly like a node body drag — it *is* that drag, run through the shared
//! [`crate::gui::canvas::drag_anchor::GroupDrag`] — and coalesced by
//! `GestureKey::SelectionDrag` into one undo entry. Since the
//! position lands in `Document` (and `Scene` rebuilds) before the record
//! pass, there's no separate in-flight preview to paint — the widget's
//! real position already reflects the live drag by the time
//! [`PinUi::draw_wires`]/[`PinUi::draw_pin`] run.

use std::collections::HashMap;

use glam::Vec2;
use palantir::{CurveBrush, Rect, Ui};
use scenarium::{NodeId, OutputPort};

use crate::core::document::{ItemRef, PortKind, PortRef};
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::UiAction;
use crate::gui::canvas::breaker::BreakerProbe;
use crate::gui::canvas::cull::CullRegion;
use crate::gui::canvas::drag_anchor::{GroupDrag, selected_group_positions};
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::pin_preview::{
    self, PREVIEW_HEIGHT, PREVIEW_WIDTH, pin_preview_wid, preview_image_wid, preview_title,
    refresh_badge_wid,
};
use crate::gui::canvas::wire::{Wire, WirePass};
use crate::gui::node::port_color::port_color;
use crate::gui::node::port_row::port_circle_wid;
use crate::gui::node::{RecordCtx, click_intents, set_output_pinned};
use crate::gui::pinned_output::StoredContent;
use crate::gui::scene::{GraphScene, Scene};
use crate::gui::theme::Theme;
use crate::gui::widgets::support::dot;

/// The port-relative offset (top-left corner, from the port center) a
/// pinned output's widget is placed at when it needs a sensible starting
/// point with no drag to derive one from — the context-menu pin toggle
/// (`crate::gui::node::port_row`) seeds a fresh pin's position with this;
/// dragging to create one instead starts it exactly at the port (see
/// [`PinUi::apply`]) so it visually "grows out of" the circle. Uses
/// [`Theme::floating_widget_gap`] for the same clearance every floating
/// overlay keeps from its anchor.
pub(crate) fn default_pin_offset(theme: &Theme) -> Vec2 {
    let gap = theme.floating_widget_gap;
    Vec2::new(gap, -(PREVIEW_HEIGHT + gap))
}

/// World-space rect of a pinned output's preview widget at `top_left`
/// (its [`crate::gui::scene::SceneOutput::pin_position`]) — the same fixed footprint at the
/// same absolute position [`PinUi::draw_pin`] paints at. Shared by the rubber-band
/// sweep's hit-test and [`PinUi::resolve`]'s breaker/hover geometry.
pub(super) fn pin_preview_rect(top_left: Vec2) -> Rect {
    Rect::new(top_left.x, top_left.y, PREVIEW_WIDTH, PREVIEW_HEIGHT)
}

/// The lone-member `Intent::MoveSelection` that seeds `port`'s position —
/// shared by the two places a pin can come into existence with no drag
/// already in flight to place it naturally: this file's Cmd+drag creation
/// (seeded at the port center) and `port_row`'s context-menu pin toggle
/// (seeded at `port_center + `[`default_pin_offset`]`(theme)`).
pub(crate) fn seed_pin_position_intent(port: OutputPort, position: Vec2) -> Intent {
    let key = ItemRef::Pin(port);
    Intent::MoveSelection {
        grabbed: key,
        moves: vec![(key, position)],
    }
}

/// Prepass scan: a click on a pin's header refresh chip (read from last
/// frame's response), returning the node its output came from. First hit
/// wins — one refresh per frame. Mirrors
/// [`crate::gui::node::prepass::emit_play_clicks`]: the pin UI surfaces only the
/// domain fact (which node to re-run); the canvas translates it into the
/// run command so this file never names `AppCommand`.
pub(super) fn emit_pin_refresh_clicks(ui: &Ui, graph: GraphScene<'_>) -> Option<NodeId> {
    graph
        .pinned_outputs()
        .find(|pin| {
            pin.node.runnable() && ui.response_for(refresh_badge_wid(pin.port)).left.clicked()
        })
        .map(|pin| pin.port.node_id)
}

/// Surface a click on a pinned card's image viewport as a viewer-open
/// request. The hover-only child does not capture the press, so the parent
/// still owns card selection and dragging; combining both responses narrows
/// an ordinary card click to the image area.
pub(crate) fn emit_pin_image_opens(ui: &Ui, scene: &Scene, actions: &mut Vec<UiAction>) {
    let Some(port) = scene.pinned_outputs().map(|pin| pin.port).find(|&port| {
        ui.response_for(pin_preview_wid(port)).left.clicked()
            && ui.response_for(preview_image_wid(port)).hovered
    }) else {
        return;
    };
    actions.push(UiAction::OpenImageViewer(port));
}

/// Pinned outputs' drag gesture plus this frame's resolved pin geometry.
#[derive(Default, Debug)]
pub(crate) struct PinUi {
    /// The card drag, shared with `NodeUI`'s body drag: whichever kind of
    /// member the pointer grabbed, the whole selection moves together.
    /// Latched from either the port circle (a fresh pin) or the preview
    /// widget (a reposition); [`GroupDrag`] takes it from there.
    drag: GroupDrag,
    /// Every on-screen pin's geometry, refilled by [`Self::resolve`] once per
    /// frame and read by both halves of the pin draw.
    ///
    /// A pin paints in two places — its wire beneath the node bodies
    /// ([`Self::draw_wires`]) and its card at its own slot in the shared
    /// paint stack ([`Self::draw_pin`]). Resolving per-draw would run the
    /// cull test, the breaker hit test, and the hover lookup twice for every
    /// pin, and would record every breaker-crossed pin *twice* against the
    /// probe. Resolving once, ahead of both passes, is what keeps
    /// [`crate::gui::canvas::breaker::BreakerState`]'s "every render pass
    /// visits its own targets at most once per frame" true of pins.
    ///
    /// Carries nothing across frames — [`Self::resolve`] clears it first —
    /// but is kept rather than rebuilt so the map's allocation is reused,
    /// like `PortLayer::live`.
    pins: HashMap<OutputPort, PinGeometry>,
}

impl PinUi {
    /// Continuous per-frame drag, exactly like a node body drag: latch on a
    /// Cmd+drag off an output port's circle (creates a fresh pin) or a
    /// plain drag off an existing preview widget (repositions it), then
    /// push one `Intent::MoveSelection` every frame the drag is held.
    /// Grabbing a pin that's already part of a multi-selection drags the
    /// whole group (nodes and pins alike) together, exactly like grabbing
    /// an already-selected node does.
    /// Swept over the whole scene once per frame, not per pane: only one
    /// pointer drag can be in flight, and every key it works with
    /// (`PortRef`, `OutputPort`) is document-unique. The graph each hit
    /// belongs to comes from the node's own `owner`, so an intent lands on
    /// the right pane's target without the caller knowing which pane the
    /// press was over.
    pub(super) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        geometry: &CanvasGeometry,
        out: &mut Intents,
    ) {
        // A live drag owns the frame; only once it ends do the latch scans
        // below get a look at this frame's presses.
        if self.drag.advance(ui, scene, out) {
            return;
        }
        if ui.modifiers().ctrl
            && let Some(port_ref) = scan_port_drag_start(geometry, scene)
            && let Some(graph) = scene.owner(port_ref.node_id)
        {
            let widget_id = port_circle_wid(port_ref);
            let port = OutputPort::new(port_ref.node_id, port_ref.port_idx);
            out.for_graph(graph.target(), |out| {
                out.push(set_output_pinned(port_ref, true));
                // A brand-new pin isn't part of any selection yet — it drags
                // alone, starting exactly at the port (so it visually "grows
                // out of" the circle) and tracking the cursor from there.
                // Seeded here — rather than left to `set_output_pinned`'s zero
                // default — so this very first frame doesn't flash the widget
                // at the canvas origin before the drag below places it. Its
                // view item lands at the top of the paint stack, so no raise
                // is needed.
                if let Some(port_center) = geometry.ports.center(port_ref) {
                    out.push(seed_pin_position_intent(port, port_center));
                }
            });
            if let Some(port_center) = geometry.ports.center(port_ref) {
                let key = ItemRef::Pin(port);
                self.drag
                    .latch(key, graph.target(), vec![(key, port_center)], widget_id);
            }
        } else if let Some((port, start_position)) = scan_widget_drag_start(ui, scene)
            && let Some(graph) = scene.owner(port.node_id)
        {
            // Grabbing a pin already in the selection drags the whole
            // group (nodes + pins) together; grabbing an unselected pin
            // repositions it alone, leaving the selection untouched but —
            // like an unselected node body's drag — lifting it to the top
            // of the paint stack, so the card being placed floats over
            // what it's dragged across.
            let key = ItemRef::Pin(port);
            let start_positions = if graph.is_selected(key) {
                selected_group_positions(graph, graph.selected())
            } else {
                out.for_graph(graph.target(), |out| out.push(Intent::Raise { key }));
                vec![(key, start_position)]
            };
            self.drag
                .latch(key, graph.target(), start_positions, pin_preview_wid(port));
        }
    }

    /// Drop last frame's resolved geometry. Each visible pane appends its
    /// own pins through [`Self::resolve`] during the record, so the clear
    /// can't live there — it would leave only the last pane's pins.
    pub(super) fn begin_frame(&mut self) {
        self.pins.clear();
    }

    /// Refill [`Self::pins`] with every pinned output the cull region keeps,
    /// recording each breaker-crossed one against `probe` exactly once. Runs
    /// before the wire pass takes the probe, so both draws read a settled
    /// result — and it is the only place either of them, or the breaker,
    /// looks at a pin's geometry.
    pub(super) fn resolve(
        &mut self,
        ui: &Ui,
        graph: GraphScene<'_>,
        geometry: &CanvasGeometry,
        probe: &mut BreakerProbe<'_>,
        cull: CullRegion,
    ) {
        for pin in graph.pinned_outputs() {
            let Some(port_center) = geometry.ports.center(output_port_ref(pin.port)) else {
                continue;
            };
            // `pin.pos` is the wire's far end and the card's own top-left
            // corner, one and the same point, so the wire lands exactly on
            // the port-circle glyph peeking from under that corner.
            let wire = Wire::data(port_center, pin.pos);
            let card = pin_preview_rect(pin.pos);
            if !cull.keeps_pin(card, &wire) {
                continue;
            }
            let broken = pin_targeted(probe, &wire, card);
            if broken {
                probe.mark_broken_pin(pin.port);
            }
            self.pins.insert(
                pin.port,
                PinGeometry {
                    wire,
                    box_hover: preview_hovered(ui, pin.port),
                    broken,
                },
            );
        }
    }

    /// This frame's geometry for `port`, or `None` when the output isn't
    /// pinned, its port hasn't measured yet, or the cull region dropped it —
    /// in all three cases neither half of the pin paints.
    fn get(&self, port: OutputPort) -> Option<PinGeometry> {
        self.pins.get(&port).copied()
    }

    /// Paint every pinned output's connecting bezier — called alongside
    /// [`crate::gui::canvas::connection_ui::draw`] /
    /// [`crate::gui::canvas::subscription_ui::draw`], *before* the node
    /// bodies, so a pin wire passing behind an unrelated node goes under it
    /// like any other wire, instead of drawing on top. Only the wire draws
    /// here; the port-circle glyph and the card paint at their own slot in
    /// the shared paint stack (see [`Self::draw_pin`]).
    ///
    /// Takes `pass` by shared ref, so it can't reach the breaker probe:
    /// [`Self::resolve`] already recorded whatever this pass crossed, and
    /// this half only paints the result.
    pub(super) fn draw_wires(&self, ui: &mut Ui, pass: &WirePass<'_, '_>) {
        let (theme, graph, geometry) = (pass.theme, pass.graph, pass.geometry);
        // Iterating the scene rather than `self.pins` keeps the paint order
        // deterministic; the map is a lookup, not a sequence.
        for pin in graph.pinned_outputs() {
            let Some(g) = self.get(pin.port) else {
                continue;
            };
            // A pin resolves its own `broken` (its card counts as part of
            // the glyph), so only the emphasis tier comes off the pass.
            let endpoint_hover =
                geometry.ports.is_hovered(output_port_ref(pin.port)) || g.box_hover;
            let stroke = pass
                .emphasis
                .stroke(theme.connection_width, g.broken, endpoint_hover);
            let base = port_color(theme, &pin.output.ty, PortKind::Output, false);
            let color = if stroke.broken {
                theme.colors.connection_broken
            } else {
                pass.emphasis.tint(base, stroke.hovered)
            };
            g.wire.add(ui, stroke.width, CurveBrush::Solid(color));
        }
    }

    /// Paint one pinned output's port-circle glyph + preview widget, at its
    /// own slot in the shared paint stack — called by
    /// [`crate::gui::node::NodeUI::draw_all`]'s z-order walk, interleaved
    /// with the node bodies, so the card sits above or below any node by
    /// stack position. Only the wire draws elsewhere (in the wires' own
    /// under-everything tier — see [`Self::draw_wires`]).
    ///
    /// `&self` for the resolved geometry alone; the drag gesture landed in
    /// `Document` (and was mirrored into `Scene`) back in the prepass, so
    /// the card's position is already the live one.
    pub(crate) fn draw_pin(
        &self,
        ui: &mut Ui,
        rcx: RecordCtx<'_>,
        port: OutputPort,
        out: &mut Intents,
    ) {
        let theme = rcx.theme;
        let graph = rcx.graph;
        // Absent when the cull region dropped this pin or its port hasn't
        // measured — the same decision `draw_wires` read, so the card and
        // its wire appear and disappear together.
        let Some(g) = self.get(port) else {
            return;
        };
        // A pin item only exists for a pinned output on a live node, but
        // the projection can still come up short (a missing-func stub
        // renders portless), so a failed lookup just skips the card.
        let Some(n) = graph.node(port.node_id) else {
            return;
        };
        let Some(output) = graph.outputs(n.outputs).get(port.port_idx) else {
            return;
        };
        // The data-type accent lives *only* on the port-circle
        // glyph (brightening on the widget's own hover, like a
        // real port circle) — the widget's own border stays
        // neutral (see `pin_preview::draw_widget`), so the accent
        // doesn't double up right next to itself.
        let accent = if g.broken {
            theme.colors.connection_broken
        } else {
            port_color(theme, &output.ty, PortKind::Output, g.box_hover)
        };
        // The wire's far end *is* the card's top-left corner, so the glyph
        // lands exactly where the bezier arrives.
        let top_left = g.wire.p3;
        dot(ui, top_left.x, top_left.y, theme.port_size * 0.5, accent);

        // Same broken/selected/resting decision a node body draws
        // (`Theme::card_border`), so "in the selection" reads as one
        // visual language across nodes and pin previews.
        let is_selected = rcx.is_selected(ItemRef::Pin(port));
        let border = theme.card_border(g.broken, is_selected);
        let value = rcx.run_state.pinned_outputs.entries.get(&port);
        let image = value.and_then(|value| match value {
            StoredContent::Image(image) => Some(image),
            StoredContent::Text(_) | StoredContent::Error(_) => None,
        });
        let text = image.is_none().then_some(match value {
            Some(StoredContent::Text(text) | StoredContent::Error(text)) => text.as_str(),
            Some(StoredContent::Image(_)) => "image preview unavailable",
            None => "not yet run",
        });
        let title = {
            let node_name = n.name.borrow_str();
            let output_name = output.name.borrow_str();
            preview_title(&node_name, &output_name)
        };
        let response = pin_preview::draw_widget(
            ui,
            theme,
            port,
            top_left,
            &title,
            border.color,
            border.width,
            image,
            text,
            n.runnable(),
        );
        // Click without drag → select + raise, exactly like a node body
        // click (plain replaces the selection, Shift toggles this
        // pin's membership); a drag repositions instead (handled by
        // `PinUi::apply`).
        if response.left.clicked() {
            let shift = ui.modifiers().shift;
            click_intents(shift, graph, ItemRef::Pin(port), out);
        }
    }
}

/// One pinned output's resolved geometry + breaker/hover state for this
/// frame — read by both [`PinUi::draw_wires`] (the bezier, pre-node-body) and
/// [`PinUi::draw_pin`] (the port circle + card, post-node-body), so a pin drawn in
/// two passes can't disagree with itself about `broken` or hover.
#[derive(Clone, Copy, Debug)]
struct PinGeometry {
    /// The connecting bezier: from the port center (`p0`) to the preview
    /// card's own top-left corner (`p3`), where the port-circle glyph peeks
    /// out from under it.
    wire: Wire,
    box_hover: bool,
    /// Whether the active breaker gesture crosses this pin's bezier or its
    /// widget rect. Already recorded via `probe.mark_broken_pin` by
    /// [`PinUi::resolve`] — readers only paint the alarm.
    broken: bool,
}

/// A pinned output's port as a [`PortRef`], for the [`CanvasGeometry`]
/// lookups keyed that way.
fn output_port_ref(port: OutputPort) -> PortRef {
    PortRef {
        node_id: port.node_id,
        kind: PortKind::Output,
        port_idx: port.port_idx,
    }
}

/// True if the active breaker gesture crosses the pin's glyph — either the
/// connecting bezier or the preview widget's rect (matching how a node
/// body's breaker hit-test uses its rect rather than an exact shape).
fn pin_targeted(probe: &BreakerProbe<'_>, wire: &Wire, card: Rect) -> bool {
    probe.crosses_wire(wire) || probe.crosses_rect(card)
}

/// First output port whose circle's drag started this frame, or `None`.
fn scan_port_drag_start(geometry: &CanvasGeometry, scene: &Scene) -> Option<PortRef> {
    let keys = scene
        .nodes
        .values()
        .filter(|node| node.run_available)
        .flat_map(|n| n.ports(PortKind::Output));
    geometry.ports.first_drag_started(keys)
}

/// First pinned output whose preview widget's drag started this frame, with
/// its current absolute position (the drag's start point) — or `None`.
/// Only a pinned output has a preview widget at all.
fn scan_widget_drag_start(ui: &Ui, scene: &Scene) -> Option<(OutputPort, Vec2)> {
    scene
        .pinned_outputs()
        .find(|pin| {
            ui.response_for(pin_preview_wid(pin.port))
                .left
                .drag
                .started()
        })
        .map(|pin| (pin.port, pin.pos))
}

/// `true` when the pointer is directly over `port`'s preview widget this
/// frame. Drives the bezier's endpoint-hover emphasis (like a data wire's)
/// and the port-circle glyph's own direct-hover tint (like a real port
/// circle's).
fn preview_hovered(ui: &Ui, port: OutputPort) -> bool {
    ui.response_for(pin_preview_wid(port)).hovered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::document::GraphRef;
    use crate::core::document::Viewport;
    use crate::gui::canvas::breaker::{BreakerState, cubic_point};
    use crate::gui::scene::SceneOutput;
    use crate::gui::scene::internals::scene_node_stub;
    use common::Span;
    use palantir::PointerButton;
    use scenarium::DataType;

    /// A `BreakerProbe` wrapping `state`, at the origin — every test here
    /// works in a local frame with no canvas offset to convert.
    fn probe_for(state: &mut BreakerState) -> BreakerProbe<'_> {
        BreakerProbe {
            origin: Vec2::ZERO,
            state: Some(state),
        }
    }

    fn box_rect_at(top_left: Vec2) -> Rect {
        Rect::new(top_left.x, top_left.y, PREVIEW_WIDTH, PREVIEW_HEIGHT)
    }

    /// A one-node scene whose sole output is pinned at `top_left`, plus a
    /// `CanvasGeometry` with that output's port circle seeded at the origin.
    fn pinned_fixture(ui: &mut Ui, top_left: Vec2) -> (Scene, CanvasGeometry, OutputPort) {
        let node_id = NodeId::unique();
        let mut node = scene_node_stub(ui, node_id, Vec2::ZERO);
        node.outputs = Span::new(0, 1);
        let scene = Scene::with_nodes([node]).with_outputs([SceneOutput {
            name: ui.intern("out"),
            description: ui.intern(""),
            ty: DataType::Int,
            pin_position: Some(top_left),
        }]);
        let port = OutputPort::new(node_id, 0);
        let mut geometry = CanvasGeometry::default();
        geometry
            .ports
            .seed_center(output_port_ref(port), Vec2::ZERO);
        (scene, geometry, port)
    }

    /// A cull region that keeps everything (the canvas hasn't measured).
    fn no_cull() -> CullRegion {
        CullRegion::from_canvas(None, Vec2::ZERO, &Viewport::default())
    }

    #[test]
    fn resolve_records_a_breaker_crossed_pin_exactly_once() {
        // A pin paints in two passes — its wire under the node bodies, its
        // card at its own z-order slot — and both now read this one
        // resolution. Each pass used to run its own hit test and call
        // `mark_broken_pin`, so one scribble across one pin queued *two*
        // unpin intents on release, falsifying `BreakerState`'s "every
        // render pass visits its own targets at most once per frame".
        //
        // Neither reader can mark any more (no `&mut` probe reaches either,
        // so the type system settles that half). What's left to pin down is
        // the resolver's own discipline: one recorded cut per crossed pin,
        // including when the scribble crosses *both* halves of the glyph and
        // the two hit tests both fire.
        let mut ui = Ui::default();
        let top_left = Vec2::new(300.0, 0.0);
        let (scene, geometry, port) = pinned_fixture(&mut ui, top_left);
        let card = box_rect_at(top_left);
        // The bezier is flat along y = 0 from x = 0 to x = 300 (every
        // control point sits on that line); the card spans x 300..580.
        let wire = Wire::data(Vec2::ZERO, top_left);
        let mid = cubic_point(wire.p0, wire.p1, wire.p2, wire.p3, 0.53);

        let cases = [
            // Vertical stroke through the bezier alone, left of the card.
            (
                "wire only",
                mid + Vec2::new(0.0, -80.0),
                mid + Vec2::new(0.0, 80.0),
                false,
            ),
            // Diagonal crossing the bezier at (175, 0) — where the stroke's
            // y reaches 0 — and ending at (400, 150), inside the card.
            (
                "wire and card",
                Vec2::new(100.0, -50.0),
                Vec2::new(400.0, 150.0),
                true,
            ),
        ];
        for (label, from, to, hits_card) in cases {
            let mut breaker = BreakerState::start(GraphRef::Main, from, PointerButton::Right);
            breaker.add_point(to);
            {
                // Precondition: the second case really does hit both halves,
                // so "exactly one" below isn't just the wire path again.
                let probe = probe_for(&mut breaker);
                assert!(probe.crosses_wire(&wire), "{label}: must cross the bezier");
                assert_eq!(probe.crosses_rect(card), hits_card, "{label}: card overlap");
            }
            let mut pin_ui = PinUi::default();
            {
                let mut probe = probe_for(&mut breaker);
                pin_ui.resolve(&ui, scene.only_graph(), &geometry, &mut probe, no_cull());
            }

            let g = pin_ui.get(port).expect("an unculled pin resolves");
            assert!(g.broken, "{label}: the scribble crosses this pin");
            assert_eq!(
                g.wire.p3, top_left,
                "{label}: the wire lands on the card's corner"
            );
            assert_eq!(
                breaker.broken_pins(),
                [port],
                "{label}: one resolution per frame is one recorded cut",
            );
        }
    }

    #[test]
    fn resolve_drops_a_culled_pin_and_leaves_it_uncut() {
        // A cull region far from the pin drops it from the map entirely, so
        // neither half paints — and the breaker can't have crossed it, since
        // the scribble is always on-screen. Same fixture and same scribble
        // as the test above, so the cull region is the only thing that
        // differs between "cut" and "not cut".
        let mut ui = Ui::default();
        let top_left = Vec2::new(300.0, 0.0);
        let (scene, geometry, port) = pinned_fixture(&mut ui, top_left);
        let wire = Wire::data(Vec2::ZERO, top_left);
        let mid = cubic_point(wire.p0, wire.p1, wire.p2, wire.p3, 0.53);
        let mut breaker = BreakerState::start(
            GraphRef::Main,
            mid + Vec2::new(0.0, -80.0),
            PointerButton::Right,
        );
        breaker.add_point(mid + Vec2::new(0.0, 80.0));

        // Origin (-5000, -5000) puts the 10×10 visible rect at world
        // (5000, 5000)..(5010, 5010) (+16 margin) — nowhere near the pin's
        // wire hull (x 0..300) or its card (x 300..580).
        let far = CullRegion::from_canvas(
            Some(Rect::new(0.0, 0.0, 10.0, 10.0)),
            Vec2::new(-5000.0, -5000.0),
            &Viewport::default(),
        );
        let mut pin_ui = PinUi::default();
        {
            let mut probe = probe_for(&mut breaker);
            pin_ui.resolve(&ui, scene.only_graph(), &geometry, &mut probe, far);
        }

        assert!(
            pin_ui.get(port).is_none(),
            "an off-screen pin resolves to nothing"
        );
        assert!(
            breaker.broken_pins().is_empty(),
            "and an off-screen pin can't be cut",
        );
    }

    #[test]
    fn pin_targeted_hits_the_preview_box_but_not_empty_space() {
        let theme = Theme::default();
        let port_center = Vec2::ZERO;
        let top_left = port_center + default_pin_offset(&theme);
        let wire = Wire::data(port_center, top_left);
        let rect = box_rect_at(top_left);
        let box_center = top_left + Vec2::new(PREVIEW_WIDTH, PREVIEW_HEIGHT) * 0.5;

        let mut hit = BreakerState::start(GraphRef::Main, box_center, PointerButton::Right);
        assert!(
            pin_targeted(&probe_for(&mut hit), &wire, rect),
            "a breaker sample landing dead-center in the preview widget must register"
        );

        let mut miss = BreakerState::start(
            GraphRef::Main,
            Vec2::new(1000.0, 1000.0),
            PointerButton::Right,
        );
        assert!(
            !pin_targeted(&probe_for(&mut miss), &wire, rect),
            "a breaker far from the glyph must not register"
        );
    }

    #[test]
    fn pin_targeted_hits_the_connecting_bezier() {
        // Synthetic, well-separated points (rather than `default_pin_offset`)
        // so the bezier's midpoint is unambiguously clear of the box rect —
        // this test exercises the bezier-crossing path, not the box-rect one.
        let port_center = Vec2::ZERO;
        let top_left = Vec2::new(300.0, 0.0);
        let wire = Wire::data(port_center, top_left);
        let rect = box_rect_at(top_left);
        let mid = cubic_point(wire.p0, wire.p1, wire.p2, wire.p3, 0.53);

        let mut state = BreakerState::start(
            GraphRef::Main,
            mid + Vec2::new(0.0, -80.0),
            PointerButton::Right,
        );
        state.add_point(mid + Vec2::new(0.0, 80.0));
        assert!(pin_targeted(&probe_for(&mut state), &wire, rect));
    }
}
