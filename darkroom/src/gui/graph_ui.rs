use common::FloatExt;
use glam::Vec2;
use palantir::{
    Background, Configure, Panel, PointerButton, Sense, Sizing, TranslateScale, Ui, WidgetId,
};
use std::collections::BTreeSet;

use crate::app::AppContext;
use crate::gui::background::CanvasBackground;
use crate::gui::breaker::BreakerUI;
use crate::gui::connection_ui::ConnectionUI;
use crate::gui::new_node_ui::NewNodeUi;
use crate::gui::node_ui::{NodeUI, RecordCtx, emit_port_disconnects};
use crate::gui::port_frame::PortFrame;
use crate::gui::selection_ui::SelectionUI;
use crate::gui::{PortKind, PortRef};
use crate::intent::Intent;
use crate::scene::{Scene, SceneNode};

/// Bounds on the canvas zoom factor. Pinch / scroll-zoom deltas
/// multiply in; clamped each frame so pathological gestures can't
/// drive it to 0 (which would make the inverse transform explode) or
/// to a value so large that the world coordinates underflow.
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 5.0;

/// Per-pixel base for converting wheel / touchpad scroll into a
/// multiplicative zoom factor. Tuned so a single classic wheel notch
/// (~16-20 logical px after palantir's line→pixel conversion) yields
/// roughly a 4-5% zoom step, while a fast touchpad swipe (~50-100 px
/// in one frame) stays a controlled ~13-22% step. Lower → slower
/// zoom, higher → snappier but jumps badly on touchpad.
const SCROLL_ZOOM_BASE: f32 = 1.0025;

/// Canvas-level UI scope: owns the port-widget-id cache, the
/// `NodeUI` that renders every graph node, and the manual pan/zoom
/// transform applied to the inner canvas. `frame` reads palantir's
/// pointer-event stream (drag on the outer canvas → pan, wheel/pinch
/// → zoom-about-cursor) and writes the result into [`Scene::pan`] /
/// [`Scene::zoom`], which then drive the inner canvas's
/// `TranslateScale`.
///
/// **Bare-canvas gesture arbitration.** Several sub-controllers each
/// poll the *same* outer-canvas response ([`outer_canvas_widget_id`])
/// and self-select on a button/modifier guard. The guards are kept
/// mutually exclusive by convention — there's no central dispatcher, so
/// keep them disjoint when editing:
/// - **Middle-drag** → pan ([`GraphUI::emit_pan_zoom`]).
/// - **Wheel / pinch** → zoom-about-cursor (same).
/// - **Plain LMB-drag** (no modifier) → rubber-band select (`SelectionUI`).
/// - **Ctrl+LMB-drag** or **RMB-drag** → connection breaker (`BreakerUI`).
/// - **RMB-click** (not drag) → new-node popup (`NewNodeUi`).
///
/// Node panels and port circles live in the *inner* canvas and hit-test
/// first, so these only fire when a gesture falls through to bare canvas.
#[derive(Default, Debug)]
pub struct GraphUI {
    background: CanvasBackground,
    port_frame: PortFrame,
    /// In-flight gesture controllers. Grouped so a tab switch can reset
    /// *all* of them in one assignment (`clear_gestures`) without the
    /// caller enumerating each — and so the persistent caches
    /// (`background`, `port_frame`) sitting beside this field survive by
    /// construction.
    gestures: Gestures,
}

/// The resettable, one-gesture-lifetime controllers. Everything here is
/// dropped on a tab switch; nothing here is a cross-frame cache.
#[derive(Default, Debug)]
struct Gestures {
    node_ui: NodeUI,
    breaker_ui: BreakerUI,
    connection_ui: ConnectionUI,
    new_node_ui: NewNodeUi,
    selection_ui: SelectionUI,
    /// `Scene::pan` snapshot captured at the frame the active pan-drag
    /// latched. While the drag is active, `scene.pan = anchor +
    /// drag_delta`. Input bookkeeping (lifetime = one gesture), not
    /// viewport state.
    pan_anchor: Option<Vec2>,
}

impl GraphUI {
    /// Drop all in-flight gesture state while **keeping** cross-frame
    /// caches — notably `PortFrame`'s port-offset table, so connections
    /// still anchor on the first frame after a tab switch. Called when
    /// the active tab changes.
    pub fn clear_gestures(&mut self) {
        self.gestures = Gestures::default();
    }

    /// Pre-record pass — see
    /// [`crate::gui::node_ui::NodeUI::prepass`]. Every input-derived
    /// intent that can change layout is emitted here, *before* the
    /// record, so its effect is applied to `Document` by the pre-record
    /// drain and Pass A records the settled layout:
    ///
    /// - pan/zoom (`emit_pan_zoom` → `Intent::SetViewport`),
    /// - node drag (`node_ui.prepass` → `Intent::MoveNodes`),
    /// - connection commit (`connection_ui.apply` → `Intent::SetInput`).
    ///
    /// Connection commit specifically *must* be here: binding an input
    /// that had a const value removes its inline editor and resizes the
    /// node. If committed during the record (post-record drain), Pass A
    /// records the pre-resize layout and the relayout's Pass B rebuilds
    /// `PortFrame` from that stale cascade — the new connection floats
    /// to the old port. Committing pre-record makes `cascade_A` the
    /// resized layout, so Pass B anchors the curve correctly with no
    /// extra frame. `PortFrame` is rebuilt here (and reused by `frame`)
    /// because the commit reads it. Navigation (tab/open) is handled
    /// separately, before this, so the target is already fixed here.
    pub fn prepass(&mut self, ui: &mut Ui, scene: &Scene, out: &mut Vec<Intent>) {
        self.emit_pan_zoom(ui, scene, out);
        self.gestures.node_ui.prepass(ui, scene, out);
        emit_port_disconnects(ui, scene, out);
        self.port_frame.rebuild(ui, scene);
        self.gestures
            .connection_ui
            .apply(ui, scene, &self.port_frame, out);
    }

    pub fn frame(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &mut Scene,
        out: &mut Vec<Intent>,
    ) {
        // Pan/zoom was already folded into the document in `prepass`
        // and mirrored into `scene` by `Scene::rebuild`, so the
        // transform below reads the up-to-date viewport directly.
        // Click on bare canvas (node panels hit-test first, so this
        // only fires when the click missed every node) clears the
        // selection. Skip when nothing is selected so we don't pollute
        // the undo stack with no-op `SetSelection` entries every time
        // the user clicks the empty canvas. A *drag* on bare canvas is
        // the rubber band (handled by `selection_ui`), not a click.
        if !scene.selected_nodes.is_empty() && ui.response_for(outer_canvas_widget_id()).clicked {
            out.push(Intent::SetSelection {
                to: BTreeSet::new(),
            });
        }
        // `PortFrame` was already rebuilt in `prepass` against the active
        // graph's scene — `App` rebuilds the scene *before* prepass on the
        // frame a tab becomes active, so prepass never sees a stale graph,
        // and the offset cache fills in port centers for nodes that hadn't
        // recorded yet. Reuse it here; no second rebuild needed.
        self.gestures.selection_ui.apply(ui, scene, out);
        self.gestures.breaker_ui.apply(ui, scene, out);
        self.gestures.new_node_ui.apply(ui, ctx, scene, out);
        // Bake the snap target into `PortFrame.hovered` so node_ui's
        // port_row picks up the hover color via the same lookup it
        // uses for ordinary mouse-over. `response.hovered` is
        // suppressed on every widget except the drag-capture owner
        // while a drag is live, so without this override the
        // snapped-but-not-captured target stays at its idle color.
        if let Some(snap) = self.gestures.connection_ui.snap_port() {
            self.port_frame.set_hovered(snap);
        }

        let Self {
            background,
            port_frame,
            gestures:
                Gestures {
                    node_ui,
                    breaker_ui,
                    connection_ui,
                    new_node_ui: _,
                    selection_ui,
                    pan_anchor: _,
                },
        } = self;
        let pan_val = scene.pan;
        let zoom_val = scene.zoom;

        // Outer canvas: covers the whole pane, paints the canvas
        // background, owns the input routing for empty-canvas
        // gestures. Senses:
        // - `DRAG`: middle-button canvas pan (graph-editor
        //   convention; left-drag is reserved for rubber-band
        //   selection once that lands). Pulled via
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
            .id(outer_canvas_widget_id())
            .size((Sizing::FILL, Sizing::FILL))
            .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL | Sense::PINCH)
            .clip_rect()
            .background(Background {
                fill: ctx.theme.canvas_bg.into(),
                ..Default::default()
            })
            .show(ui, |ui| {
                // Dotted backdrop in screen space, beneath the inner
                // (transformed) canvas — so it paints under everything.
                background.draw(ui, ctx, pan_val, zoom_val);
                Panel::canvas()
                    .id(inner_canvas_widget_id())
                    .size((Sizing::FILL, Sizing::FILL))
                    .transform(TranslateScale::new(pan_val, zoom_val))
                    .show(ui, |ui| {
                        // Inner canvas's pre-transform origin. Shapes
                        // and child node panels recorded inside this
                        // closure share the inner canvas's transform
                        // (palantir's `Panel::transform` applies to
                        // the body: child subtrees AND direct
                        // shapes), so port `layout_rect`s and bezier
                        // endpoints stay aligned at every zoom.
                        let canvas_origin = ui
                            .response_for(inner_canvas_widget_id())
                            .layout_rect
                            .map(|r| r.min)
                            .unwrap_or(Vec2::ZERO);
                        // Painted first so it sits beneath the
                        // connections and node bodies.
                        selection_ui.draw(ui, ctx);
                        {
                            let mut probe = breaker_ui.probe(canvas_origin);
                            connection_ui.draw(ui, ctx, scene, port_frame, &mut probe);
                            let rcx = RecordCtx {
                                theme: ctx.theme,
                                scene,
                                port_frame,
                            };
                            node_ui.draw_all(ui, rcx, &mut probe, out);
                        }
                        breaker_ui.draw(ui, ctx);
                        connection_ui.draw_in_flight(ui, ctx, scene, port_frame, canvas_origin);
                    });
            });
    }

    /// Read the outer canvas's current-frame response, compute the
    /// target viewport, and emit an `Intent::SetViewport` when it
    /// changed. The intent (not a direct write) is the only thing that
    /// mutates the document's viewport — so pan/zoom rides the same
    /// undo path as every other edit, and the undo stack coalesces a
    /// continuous gesture into one entry via `GestureKey::Viewport`.
    /// Three independent sources:
    ///
    /// - **Middle-button drag** (`Sense::DRAG` +
    ///   `Ui::drag_delta_by`): canvas pan. Anchor on `drag_started_by`,
    ///   then `pan = anchor + delta` until release. Left-drag is
    ///   intentionally NOT routed to pan so it stays free for future
    ///   rubber-band selection.
    /// - **Scroll** (`Sense::SCROLL`): mouse wheel / touchpad swipe →
    ///   zoom-about-cursor (graph-editor convention: Figma / Blender
    ///   node editor / ComfyUI). Vertical delta only; horizontal is
    ///   ignored. Palantir ingests the scroll delta already-negated
    ///   so `+y` means "scroll content down" → zoom out, `-y` (wheel
    ///   up) → zoom in.
    /// - **Pinch** (`Sense::PINCH`): zoom-about-cursor using the
    ///   `Response::pointer_local` pivot.
    fn emit_pan_zoom(&mut self, ui: &Ui, scene: &Scene, out: &mut Vec<Intent>) {
        let resp = ui.response_for(outer_canvas_widget_id());
        let mut pan = scene.pan;
        let mut zoom = scene.zoom;
        if resp.drag_started_by(PointerButton::Middle) {
            self.gestures.pan_anchor = Some(scene.pan);
        }
        match (
            self.gestures.pan_anchor,
            resp.drag_delta_by(PointerButton::Middle),
        ) {
            (Some(anchor), Some(d)) => pan = anchor + d,
            (Some(_), None) => self.gestures.pan_anchor = None,
            _ => {}
        }
        if resp.scroll_pixels != Vec2::ZERO {
            pan -= resp.scroll_pixels;
        }
        if resp.scroll_lines.y.abs() > f32::EPSILON
            && let Some(pivot) = resp.pointer_local
        {
            let line_px = ui.theme.text.line_height_for(ui.theme.text.font_size_px);
            zoom_about(
                &mut pan,
                &mut zoom,
                pivot,
                scroll_to_zoom_factor(resp.scroll_lines.y * line_px),
            );
        }
        if (resp.zoom_factor - 1.0).abs() > f32::EPSILON
            && let Some(pivot) = resp.pointer_local
        {
            zoom_about(&mut pan, &mut zoom, pivot, resp.zoom_factor);
        }
        // Only emit when the gesture actually moved the viewport
        // (approx compare — exact float `!=` would emit on sub-epsilon
        // jitter). The `SetViewport` undo step is also `is_noop`-
        // filtered in `drain_intents`; this just skips the build on
        // idle frames.
        let unchanged = pan.approximately_eq(scene.pan) && zoom.approximately_eq(scene.zoom);
        if !unchanged {
            out.push(Intent::SetViewport { pan, scale: zoom });
        }
    }
}

/// Every `PortRef` of `node` on the given side, in port order. Single
/// source for the "iterate a node's ports by kind" loop that
/// `PortFrame::rebuild` and the connection scans all need, so scan order
/// and paint order can't drift apart.
pub(super) fn node_ports<'a>(
    scene: &'a Scene,
    node: &'a SceneNode,
    kind: PortKind,
) -> impl Iterator<Item = PortRef> + 'a {
    let span = match kind {
        PortKind::Input => node.inputs,
        PortKind::Output => node.outputs,
    };
    let count = scene.ports(span).len();
    (0..count).map(move |port_idx| PortRef {
        node_id: node.id,
        kind,
        port_idx,
    })
}

/// Outer-canvas-local coords → inner-canvas pre-transform world
/// coords. Inner canvas applies `TranslateScale::new(pan, zoom)`,
/// so `outer = pan + zoom * world`.
pub(super) fn to_world(outer_local: Vec2, scene: &Scene) -> Vec2 {
    let zoom = if scene.zoom > 0.0 { scene.zoom } else { 1.0 };
    (outer_local - scene.pan) / zoom
}

/// Multiply `zoom` by `factor` while holding the pre-transform point
/// under `pivot_local` fixed in the outer canvas. Operates on the
/// caller's local `(pan, zoom)` so the gesture can fold several inputs
/// before emitting one `Intent::SetViewport`. Standard zoom-about-
/// cursor algebra: world point under cursor = `(pivot - pan) / zoom`;
/// choose new pan so that same world point stays under the same screen
/// pixel after scaling. Clamps to `[MIN_ZOOM, MAX_ZOOM]`; ignores
/// non-finite / non-positive factors.
fn zoom_about(pan: &mut Vec2, zoom: &mut f32, pivot_local: Vec2, factor: f32) {
    if !factor.is_finite() || factor <= 0.0 {
        return;
    }
    let new_zoom = (*zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    let effective = new_zoom / *zoom;
    *pan = pivot_local - (pivot_local - *pan) * effective;
    *zoom = new_zoom;
}

/// Map a one-frame vertical scroll delta (in logical px, palantir's
/// "advance offset forward" sign convention — `+y` = scroll content
/// down) to a multiplicative zoom factor. Negative `delta_y` (wheel
/// up) zooms in (`factor > 1`); positive (wheel down) zooms out
/// (`factor < 1`). Pure function so it can be unit-tested without
/// spinning up a UI.
fn scroll_to_zoom_factor(delta_y: f32) -> f32 {
    SCROLL_ZOOM_BASE.powf(-delta_y)
}

/// Stable id for the outer (pan-capture) canvas. `auto_stable` mixes
/// `file!()`/`line!()` so calls from different source lines stay
/// distinct; here we only need the id to survive between frames.
pub(super) const fn outer_canvas_widget_id() -> WidgetId {
    WidgetId::auto_stable()
}

/// Stable id for the inner (transformed) canvas. Used as the widget
/// seed and for resolving the canvas's pre-transform origin in
/// connection draws.
const fn inner_canvas_widget_id() -> WidgetId {
    WidgetId::auto_stable()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_to_zoom_factor_zero_delta_is_identity() {
        // No scroll event → no zoom change. Bit-exact because
        // `1.0025_f32.powf(0.0)` returns exactly `1.0` by f32 spec.
        assert_eq!(scroll_to_zoom_factor(0.0), 1.0);
    }

    #[test]
    fn scroll_to_zoom_factor_wheel_up_zooms_in() {
        // One classic wheel notch up after palantir line→pixel
        // conversion lands around `-line_px` (theme default ≈ 18 px,
        // sign-flipped at ingest). Round-trip check: a typical wheel
        // notch produces a > 1.0 factor; magnitude is the documented
        // `SCROLL_ZOOM_BASE^|delta|`.
        let f = scroll_to_zoom_factor(-18.0);
        assert!(f > 1.0, "wheel up must zoom in, got factor {f}");
        // Hand-computed: 1.0025^18 ≈ 1.04604.
        let expected = SCROLL_ZOOM_BASE.powf(18.0);
        assert!(
            (f - expected).abs() < 1e-6,
            "factor {f} != expected {expected}",
        );
    }

    #[test]
    fn scroll_to_zoom_factor_wheel_down_zooms_out() {
        // Mirrored notch in the other direction: factor < 1, and
        // multiplied with the up-notch factor produces ~1.0 (the two
        // are reciprocals modulo float error).
        let f_down = scroll_to_zoom_factor(18.0);
        let f_up = scroll_to_zoom_factor(-18.0);
        assert!(f_down < 1.0, "wheel down must zoom out, got {f_down}");
        let product = f_down * f_up;
        assert!(
            (product - 1.0).abs() < 1e-6,
            "opposite-direction factors must reciprocate, got product {product}",
        );
    }

    #[test]
    fn scroll_to_zoom_factor_scales_monotonically_with_magnitude() {
        // 4 notches up zooms more aggressively than 1 notch up.
        let one = scroll_to_zoom_factor(-18.0);
        let four = scroll_to_zoom_factor(-72.0);
        assert!(
            four > one,
            "larger-magnitude up-scroll must produce larger factor; one={one}, four={four}",
        );
        // 4 × notch = (single notch factor) ^ 4 by exponent law.
        let expected_four = one.powi(4);
        assert!(
            (four - expected_four).abs() < 1e-5,
            "factor for 4 notches {four} != single^4 {expected_four}",
        );
    }

    #[test]
    fn zoom_about_holds_pivot_invariant() {
        // The point under the pivot in world space (i.e. in
        // pre-transform inner-canvas coords) must land on the same
        // local pivot after zooming. Algebra:
        //   world_before = (pivot - pan_before) / zoom_before
        //   world_after  = (pivot - pan_after)  / zoom_after
        //   require world_before == world_after.
        let (mut pan, mut zoom) = (Vec2::new(40.0, 20.0), 1.5);
        let pivot = Vec2::new(200.0, 150.0);
        let world_before = (pivot - pan) / zoom;
        zoom_about(&mut pan, &mut zoom, pivot, 1.3);
        let world_after = (pivot - pan) / zoom;
        let drift = (world_after - world_before).length();
        assert!(
            drift < 1e-4,
            "world point under pivot drifted by {drift} (before={world_before}, after={world_after})",
        );
    }

    #[test]
    fn zoom_about_with_scroll_factor_preserves_pivot() {
        // End-to-end: a wheel scroll triggers `zoom_about` with the
        // `scroll_to_zoom_factor` output. The same pivot invariant
        // must hold regardless of which factor source the caller used.
        let (mut pan, mut zoom) = (Vec2::new(-15.0, 75.0), 0.8);
        let pivot = Vec2::new(300.0, 200.0);
        let world_before = (pivot - pan) / zoom;
        // 2 notches up.
        let factor = scroll_to_zoom_factor(-36.0);
        zoom_about(&mut pan, &mut zoom, pivot, factor);
        let world_after = (pivot - pan) / zoom;
        let drift = (world_after - world_before).length();
        assert!(drift < 1e-4, "drift {drift}");
        // Sanity: zoom did move in the expected direction (in).
        assert!(zoom > 0.8, "scroll up should grow zoom; got {zoom}");
    }

    #[test]
    fn zoom_about_clamps_to_max() {
        // Trying to zoom past `MAX_ZOOM` saturates without
        // overshooting. Pivot invariance still holds at the clamped
        // value (effective factor = MAX_ZOOM / zoom_before, not the
        // requested factor).
        let (mut pan, mut zoom) = (Vec2::new(10.0, 10.0), MAX_ZOOM * 0.9);
        let pivot = Vec2::new(100.0, 100.0);
        zoom_about(&mut pan, &mut zoom, pivot, 5.0);
        assert!(
            (zoom - MAX_ZOOM).abs() < 1e-5,
            "expected saturation at MAX_ZOOM={MAX_ZOOM}, got {zoom}",
        );
    }

    #[test]
    fn zoom_about_clamps_to_min() {
        let (mut pan, mut zoom) = (Vec2::new(10.0, 10.0), MIN_ZOOM * 1.1);
        let pivot = Vec2::new(100.0, 100.0);
        zoom_about(&mut pan, &mut zoom, pivot, 0.01);
        assert!(
            (zoom - MIN_ZOOM).abs() < 1e-5,
            "expected saturation at MIN_ZOOM={MIN_ZOOM}, got {zoom}",
        );
    }

    #[test]
    fn zoom_about_ignores_non_positive_or_non_finite_factor() {
        // Defensive: pathological factors leave the viewport unchanged.
        let pan0 = Vec2::new(5.0, 7.0);
        let zoom0 = 1.25;
        for bad in [0.0_f32, -0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let (mut pan, mut zoom) = (pan0, zoom0);
            zoom_about(&mut pan, &mut zoom, Vec2::new(50.0, 50.0), bad);
            assert_eq!(pan, pan0, "pan moved on bad factor {bad}");
            assert_eq!(zoom, zoom0, "zoom moved on bad factor {bad}");
        }
    }
}
