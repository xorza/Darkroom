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
//! exactly like a node body drag (see `NodeUI`'s `DragAnchor`/`prepass`),
//! coalesced by `GestureKey::SelectionDrag` into one undo entry. Since the
//! position lands in `Document` (and `Scene` rebuilds) before the record
//! pass, there's no separate in-flight preview to paint — the widget's
//! real position already reflects the live drag by the time
//! [`PinUi::draw_wire`]/[`PinUi::draw_pin`] run.

use std::collections::BTreeSet;

use aperture::{CurveBrush, Rect, Ui};
use glam::Vec2;
use scenarium::{NodeId, OutputPort};

use crate::core::document::{ItemRef, PortKind, PortRef};
use crate::core::edit::intent::types::Intent;
use crate::gui::UiAction;
use crate::gui::app::AppContext;
use crate::gui::canvas::breaker::BreakerProbe;
use crate::gui::canvas::cull::CullRegion;
use crate::gui::canvas::drag_anchor::{GroupDragAnchor, selected_group_positions};
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::node_ports;
use crate::gui::canvas::pin_preview::{
    self, PREVIEW_HEIGHT, PREVIEW_WIDTH, pin_preview_wid, preview_image_wid, preview_title,
    refresh_badge_wid,
};
use crate::gui::canvas::wire::{CubicHandles, WireEmphasis, add_cubic_wire, cubic_handles};
use crate::gui::node::port_color::port_color;
use crate::gui::node::port_row::port_circle_wid;
use crate::gui::node::{RecordCtx, click_intents, set_output_pinned};
use crate::gui::pinned_output::StoredContent;
use crate::gui::scene::Scene;
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
/// same absolute position [`draw_pin`](PinUi::draw_pin) paints at. Shared
/// by the rubber-band sweep's hit-test and [`resolve_pin_geometry`]'s
/// breaker/hover geometry.
pub(crate) fn pin_preview_rect(top_left: Vec2) -> Rect {
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
pub(crate) fn emit_pin_refresh_clicks(ui: &Ui, scene: &Scene) -> Option<NodeId> {
    scene
        .pinned_outputs()
        .find(|pin| ui.response_for(refresh_badge_wid(pin.port)).left.clicked())
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
    actions.push(UiAction::OpenImageViewer(PortRef {
        node_id: port.node_id,
        kind: PortKind::Output,
        port_idx: port.port_idx,
    }));
}

/// The pin (or brand-new pin) a drag latched onto, and every member moving
/// with it — `key` is the grabbed pin's port. Shares its shape with
/// `NodeUI`'s `DragAnchor` (see [`GroupDragAnchor`]): every later frame's
/// committed position is `start + drag_delta`, not a running integration
/// over the moving widget. `start_positions` includes the grabbed pin
/// itself — the port center for a freshly created pin (so it "grows out of"
/// the port circle as the user drags), or the widget's already-resolved
/// position for a reposition (so it continues from where it visually sits
/// instead of jumping). `widget_id` is captured at latch so later frames
/// can `ui.response_for(widget_id)` directly: the port circle for a fresh
/// pin, the preview widget for a reposition.
type PinDragAnchor = GroupDragAnchor<OutputPort>;

/// Pinned outputs' preview-widget drag state. Only one pin drag is ever in
/// flight, so `drag` is a single slot.
#[derive(Default, Debug)]
pub(crate) struct PinUi {
    drag: Option<PinDragAnchor>,
}

impl PinUi {
    /// Continuous per-frame drag, exactly like a node body drag: latch on a
    /// Cmd+drag off an output port's circle (creates a fresh pin) or a
    /// plain drag off an existing preview widget (repositions it), then
    /// push one `Intent::MoveSelection` every frame the drag is held.
    /// Grabbing a pin that's already part of a multi-selection drags the
    /// whole group (nodes and pins alike) together, exactly like grabbing
    /// an already-selected node does.
    pub(crate) fn apply(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        geometry: &CanvasGeometry,
        selected: &BTreeSet<ItemRef>,
        out: &mut Vec<Intent>,
    ) {
        if let Some(anchor) = self.drag.clone() {
            if !scene.nodes.contains_key(&anchor.key.node_id) {
                // Stale: the node vanished mid-drag (breaker/undo). Drop
                // rather than build an intent against a missing node.
                self.drag = None;
            } else {
                let resp = ui.response_for(anchor.widget_id);
                if resp.left.drag.started() {
                    // A fresh gesture just replaced this one on the same
                    // widget; drop rather than fire with stale start data.
                    self.drag = None;
                } else if let Some(delta) = resp.left.drag.delta() {
                    out.push(anchor.resolve(delta, ItemRef::Pin(anchor.key)));
                    return;
                } else {
                    // No delta means the drag isn't latched anymore
                    // (release or pointer-left-surface).
                    self.drag = None;
                }
            }
        }
        if ui.modifiers().ctrl
            && let Some(port_ref) = scan_port_drag_start(geometry, scene)
        {
            let widget_id = port_circle_wid(port_ref);
            out.push(set_output_pinned(port_ref, true));
            let port = OutputPort::new(port_ref.node_id, port_ref.port_idx);
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
                self.drag = Some(PinDragAnchor {
                    key: port,
                    start_positions: vec![(ItemRef::Pin(port), port_center)],
                    widget_id,
                });
            }
        } else if let Some((port, start_position)) = scan_widget_drag_start(ui, scene) {
            // Grabbing a pin already in the selection drags the whole
            // group (nodes + pins) together; grabbing an unselected pin
            // repositions it alone, leaving the selection untouched but —
            // like an unselected node body's drag — lifting it to the top
            // of the paint stack, so the card being placed floats over
            // what it's dragged across.
            let key = ItemRef::Pin(port);
            let start_positions = if selected.contains(&key) {
                selected_group_positions(scene, selected)
            } else {
                out.push(Intent::Raise { key });
                vec![(key, start_position)]
            };
            self.drag = Some(PinDragAnchor {
                key: port,
                start_positions,
                widget_id: pin_preview_wid(port),
            });
        }
    }

    /// Paint every pinned output's connecting bezier — called alongside
    /// `ConnectionUI`/`SubscriptionUI`'s wire draws, *before* the node
    /// bodies, so a pin wire passing behind an unrelated node goes under it
    /// like any other wire, instead of drawing on top. Marks a
    /// breaker-crossed pin via `probe.mark_broken_pin` for the breaker's
    /// release-frame drain — the same combined bezier-or-widget-rect test
    /// [`draw_pin`](Self::draw_pin) runs, so both halves agree on
    /// `broken` within the same frame even though they paint at different
    /// points in the pass. Only the wire draws here; the port-circle
    /// glyph and the card paint at their own slot in the shared paint
    /// stack (see [`draw_pin`](Self::draw_pin)).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_wire(
        &self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        geometry: &CanvasGeometry,
        cull: CullRegion,
        probe: &mut BreakerProbe<'_>,
        emphasis: &WireEmphasis,
    ) {
        let theme = ctx.theme;
        for pin in scene.pinned_outputs() {
            let Some(g) = resolve_pin_geometry(ui, geometry, probe, cull, pin.port, pin.pos) else {
                continue;
            };
            let port_ref = PortRef {
                node_id: pin.port.node_id,
                kind: PortKind::Output,
                port_idx: pin.port.port_idx,
            };
            let hovered =
                !g.broken && emphasis.hovered(geometry.ports.is_hovered(port_ref) || g.box_hover);
            let base = port_color(theme, &pin.output.ty, PortKind::Output, false);
            let wire_color = if g.broken {
                theme.colors.connection_broken
            } else {
                emphasis.tint(base, hovered)
            };
            let width = emphasis.width(theme.connection_width, hovered || g.broken);
            add_cubic_wire(
                ui,
                g.port_center,
                g.top_left,
                g.handles,
                width,
                CurveBrush::Solid(wire_color),
            );
        }
    }

    /// Paint one pinned output's port-circle glyph + preview widget, at
    /// its own slot in the shared paint stack — called by
    /// [`crate::gui::node::NodeUI::draw_all`]'s z-order walk, interleaved
    /// with the node bodies, so the card sits above or below any node by
    /// stack position. Only the wire draws elsewhere (in the wires' own
    /// under-everything tier — see [`draw_wire`](Self::draw_wire)).
    pub(crate) fn draw_pin(
        &mut self,
        ui: &mut Ui,
        rcx: RecordCtx<'_>,
        port: OutputPort,
        cull: CullRegion,
        probe: &mut BreakerProbe<'_>,
        out: &mut Vec<Intent>,
    ) {
        let theme = rcx.theme;
        let scene = rcx.scene;
        // A pin item only exists for a pinned output on a live node, but
        // the projection can still come up short (a missing-func stub
        // renders portless), so a failed lookup just skips the card.
        let Some(n) = scene.nodes.get(&port.node_id) else {
            return;
        };
        let Some(output) = scene.outputs(n.outputs).get(port.port_idx) else {
            return;
        };
        let Some(pin_position) = output.pin_position else {
            return;
        };
        let Some(g) = resolve_pin_geometry(ui, rcx.geometry, probe, cull, port, pin_position)
        else {
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
        dot(
            ui,
            g.top_left.x,
            g.top_left.y,
            theme.port_size * 0.5,
            accent,
        );

        // Same broken/selected/resting decision a node body draws
        // (`Theme::card_border`), so "in the selection" reads as one
        // visual language across nodes and pin previews.
        let is_selected = rcx.selected.contains(&ItemRef::Pin(g.out_port));
        let border = theme.card_border(g.broken, is_selected);
        let value = rcx.run_state.pinned_outputs.entries.get(&g.out_port);
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
            g.out_port,
            g.top_left,
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
            click_intents(shift, scene, ItemRef::Pin(g.out_port), out);
        }
    }
}

/// One pinned output's resolved geometry + breaker/hover state for this
/// frame — computed once, fed to both [`PinUi::draw_wire`] (the bezier,
/// pre-node-body) and [`PinUi::draw_pin`] (the port circle + card,
/// post-node-body) so a pin drawn in two passes still agrees on `broken`
/// and hover within the same frame.
#[derive(Debug)]
struct PinGeometry {
    out_port: OutputPort,
    port_center: Vec2,
    top_left: Vec2,
    handles: CubicHandles,
    box_hover: bool,
    /// Whether the active breaker gesture crosses this pin's bezier or its
    /// widget rect — already folded into `probe.mark_broken_pin` here, so
    /// neither caller needs to repeat that.
    broken: bool,
}

/// `top_left` is the pinned output's [`crate::gui::scene::SceneOutput::pin_position`] — the
/// wire's far end and the preview widget's own top-left corner, one and
/// the same point, so the wire lands exactly on the port-circle glyph
/// peeking from under that corner.
fn resolve_pin_geometry(
    ui: &Ui,
    geometry: &CanvasGeometry,
    probe: &mut BreakerProbe<'_>,
    cull: CullRegion,
    out_port: OutputPort,
    top_left: Vec2,
) -> Option<PinGeometry> {
    let port_ref = PortRef {
        node_id: out_port.node_id,
        kind: PortKind::Output,
        port_idx: out_port.port_idx,
    };
    let port_center = geometry.ports.center(port_ref)?;
    let handles = cubic_handles(port_center, top_left);
    let box_rect = pin_preview_rect(top_left);
    if !cull.keeps_pin(box_rect, port_center, &handles, top_left) {
        return None;
    }
    let broken = pin_targeted(probe, port_center, &handles, top_left, box_rect);
    if broken {
        probe.mark_broken_pin(out_port);
    }
    let box_hover = preview_hovered(ui, out_port);
    Some(PinGeometry {
        out_port,
        port_center,
        top_left,
        handles,
        box_hover,
        broken,
    })
}

/// True if the active breaker gesture crosses the pin's glyph — either the
/// connecting bezier or the preview widget's rect (matching how a node
/// body's breaker hit-test uses its rect rather than an exact shape).
fn pin_targeted(
    probe: &BreakerProbe<'_>,
    port_center: Vec2,
    handles: &CubicHandles,
    wire_end: Vec2,
    box_rect: Rect,
) -> bool {
    if probe.crosses_cubic(port_center, handles.p1, handles.p2, wire_end) {
        return true;
    }
    probe.crosses_rect(box_rect)
}

/// First output port whose circle's drag started this frame, or `None`.
fn scan_port_drag_start(geometry: &CanvasGeometry, scene: &Scene) -> Option<PortRef> {
    let keys = scene
        .nodes
        .values()
        .flat_map(|n| node_ports(n, PortKind::Output));
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
    use crate::gui::canvas::breaker::{BreakerState, cubic_point};
    use aperture::PointerButton;

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

    #[test]
    fn pin_targeted_hits_the_preview_box_but_not_empty_space() {
        let theme = Theme::default();
        let port_center = Vec2::ZERO;
        let top_left = port_center + default_pin_offset(&theme);
        let handles = cubic_handles(port_center, top_left);
        let rect = box_rect_at(top_left);
        let box_center = top_left + Vec2::new(PREVIEW_WIDTH, PREVIEW_HEIGHT) * 0.5;

        let mut hit = BreakerState::start(box_center, PointerButton::Right);
        assert!(
            pin_targeted(&probe_for(&mut hit), port_center, &handles, top_left, rect),
            "a breaker sample landing dead-center in the preview widget must register"
        );

        let mut miss = BreakerState::start(Vec2::new(1000.0, 1000.0), PointerButton::Right);
        assert!(
            !pin_targeted(&probe_for(&mut miss), port_center, &handles, top_left, rect),
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
        let handles = cubic_handles(port_center, top_left);
        let rect = box_rect_at(top_left);
        let mid = cubic_point(port_center, handles.p1, handles.p2, top_left, 0.53);

        let mut state = BreakerState::start(mid + Vec2::new(0.0, -80.0), PointerButton::Right);
        state.add_point(mid + Vec2::new(0.0, 80.0));
        assert!(pin_targeted(
            &probe_for(&mut state),
            port_center,
            &handles,
            top_left,
            rect
        ));
    }
}
