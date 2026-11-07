//! The ties on the canvas as widgets: each band paints, and records a hover and press target the
//! size of the band and its head; a selected one records its knob too. A hover lights the tie's
//! wires, a press selects it and starts the group drag, and the knob's drag turns the band.

use std::rc::Rc;

use glam::Vec2;
use palantir::{
    Configure, Modifiers, Panel, PointerAction, PointerButton, PointerEdge, Sense, Sizing, Ui,
};

use crate::core::document::item_id::ItemId;
use crate::core::document::tie::{TieAngle, TieId};
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::click_mode::ClickMode;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::draw::tie::band::{BandAxes, BandInk, BandLook, KNOB_HIT_RADIUS};
use crate::gui::pane::graph_ui::draw_ctx::DrawCtx;
use crate::gui::pane::graph_ui::gesture::group_drag::{self, Grab, GrabStart};
use crate::gui::pane::graph_ui::gesture::tie_ui::tie_probe::TieProbe;
use crate::gui::pane::graph_ui::scene::tie_layout::TieLayout;
use crate::gui::pane::graph_ui::unsettled_ctx::UnsettledCtx;
use crate::gui::pane::graph_ui::wid;

/// The step a knob turned with Shift rounds to, in degrees, as design tools snap.
const SNAP_DEGREES: f32 = 15.0;

/// The ties' own gesture, the knob drag that turns one.
#[derive(Default, Debug)]
pub(crate) struct TieWidgets {
    knob: Option<KnobDrag>,
}

/// A knob held: the tie it turns, the angle to put back on Esc, and the undo gesture its frames
/// fold into.
#[derive(Clone, Copy, Debug)]
struct KnobDrag {
    tie: TieId,
    from: TieAngle,
    gesture: GestureId,
}

/// The canvas pass the ties record in: the draw context, the layout of their bands, and the tie
/// line they light under.
#[derive(Debug)]
pub(crate) struct TiePass<'a, 't> {
    pub(crate) draw_ctx: DrawCtx<'a>,
    pub(crate) ties: &'a TieLayout,
    pub(crate) line: &'a mut TieProbe<'t>,
}

/// Whether a knob turned with Shift snaps to [`SNAP_DEGREES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AngleSnap {
    Free,
    Steps,
}

impl TieWidgets {
    /// Drop the knob in flight.
    pub(crate) const fn reset(&mut self) {
        self.knob = None;
    }

    pub(crate) const fn in_flight(&self) -> bool {
        self.knob.is_some()
    }

    /// Pre-record: turn a held knob's band to the pointer. The frames of one knob drag fold into
    /// one undo entry; Esc puts the angle back.
    pub(crate) fn input(&mut self, ui: &mut Ui, stage: &mut Stage<'_, UnsettledCtx<'_>>) {
        let ctx = stage.ctx;
        let out = &mut *stage.out;
        let Some(knob) = self.knob.as_ref().copied() else {
            return;
        };
        let turn = |angle| GraphIntent::SetTieAngle {
            tie: knob.tie,
            angle,
            gesture: Some(knob.gesture),
        };
        if ctx.cancelled() {
            out.push_graph(turn(knob.from));
            self.knob = None;
            return;
        }
        let graph_ctx = ctx.graph_ctx();
        let center = graph_ctx.view().ties.get(&knob.tie).map(|tie| tie.pos);
        let (Some(center), Some(_)) = (
            center,
            ui.response_for(wid::tie_knob(knob.tie)).left.drag.delta(),
        ) else {
            self.knob = None;
            return;
        };
        let snap = if ui.modifiers().contains(Modifiers::SHIFT) {
            AngleSnap::Steps
        } else {
            AngleSnap::Free
        };
        if let Some(pointer) = ui.response_for(canvas::outer_wid()).pointer_local {
            let offset = graph_ctx.camera().to_world(pointer) - center;
            out.push_graph(turn(TieAngle::Pinned(knob_angle(offset, snap))));
        }
    }

    /// A knob's left edges: its double-click frees the angle, and its drag latches the turn.
    pub(crate) fn act_on_knob(
        &mut self,
        tie: TieId,
        action: PointerAction,
        stage: &mut Stage<'_, CanvasCtx<'_>>,
    ) {
        if action.button != PointerButton::Left {
            return;
        }
        match action.edge {
            PointerEdge::Clicked { count: 2 } => {
                stage.out.push_graph(GraphIntent::SetTieAngle {
                    tie,
                    angle: TieAngle::Auto,
                    gesture: None,
                });
            }
            PointerEdge::DragStarted => {
                self.knob = Some(KnobDrag {
                    tie,
                    from: stage.ctx.graph_ctx().view().ties[&tie].angle,
                    gesture: stage.out.open_gesture(),
                });
            }
            _ => {}
        }
    }
}

impl TiePass<'_, '_> {
    /// Paint each band with a strand on screen and record its targets: the hover and press
    /// target, and on a selected tie its knob. Runs after the wires and before the node bodies,
    /// so a body covers a band as it covers wires, and wins a press over it.
    pub(crate) fn record(&mut self, ui: &mut Ui) {
        let draw_ctx = self.draw_ctx;
        let theme = draw_ctx.theme();
        for band in self
            .ties
            .bands()
            .iter()
            .filter(|band| band.strand_count() > 0)
        {
            let axes = BandAxes::of(band);
            if !draw_ctx.cull().keeps_rect(axes.bounds(theme.stroke.fine)) {
                continue;
            }
            let item = ItemId::Tie(band.tie);
            let selected = draw_ctx.is_selected(item);
            let ink = if self.line.cross_band(band.tie, axes.reach()) {
                BandInk::Crossed
            } else if selected || ui.response_for(wid::tie_body(band.tie)).hovered() {
                BandInk::Lit
            } else {
                BandInk::Rest
            };
            axes.paint(
                ui,
                theme,
                BandLook {
                    ink,
                    selected,
                    pinned: matches!(band.angle, TieAngle::Pinned(_)),
                },
            );
            let hit = axes.hit_rect();
            Panel::zstack()
                .id(wid::tie_body(band.tie))
                .at(hit.min)
                .size((Sizing::fixed(hit.size.w), Sizing::fixed(hit.size.h)))
                .sense(Sense::CLICK | Sense::DRAG)
                .show(ui, |_| {});
            if selected {
                record_knob(ui, band.tie, axes.knob_center());
            }
        }
    }
}

/// Record the knob of a selected tie, whose edges [`TieWidgets::act_on_knob`] reads.
fn record_knob(ui: &mut Ui, tie: TieId, center: Vec2) {
    Panel::zstack()
        .id(wid::tie_knob(tie))
        .at(center - Vec2::splat(KNOB_HIT_RADIUS))
        .size(KNOB_HIT_RADIUS * 2.0)
        .sense(Sense::CLICK | Sense::DRAG)
        .show(ui, |_| {});
}

/// The group drag of a pressed tie: with the selection when the tie is in it, else alone,
/// selected, with the ties inside the moving nodes, which are none.
pub(crate) fn grab(graph_ctx: GraphCtx<'_>, tie: TieId) -> GrabStart {
    let item = ItemId::Tie(tie);
    let (members, joined) = if graph_ctx.is_selected(item) {
        (group_drag::selected_group(graph_ctx), Vec::new())
    } else {
        let selects = GraphIntent::click(ClickMode::Replace, graph_ctx.selected(), item);
        let alone = Rc::from([DragStart {
            item,
            pos: graph_ctx.view().ties[&tie].pos,
        }]);
        (alone, selects.collect())
    };
    GrabStart {
        grab: Grab {
            item,
            members,
            handle: wid::tie_body(tie),
        },
        joined,
    }
}

/// The band angle a knob held `offset` from the tie's center sets, in degrees from `+x` toward
/// `+y`, in `[0, 360)`: the pointer's own angle, or with [`AngleSnap::Steps`] the nearest
/// [`SNAP_DEGREES`] step.
pub(crate) fn knob_angle(offset: Vec2, snap: AngleSnap) -> f32 {
    let degrees = offset.y.atan2(offset.x).to_degrees();
    let degrees = match snap {
        AngleSnap::Free => degrees,
        AngleSnap::Steps => (degrees / SNAP_DEGREES).round() * SNAP_DEGREES,
    };
    let wrapped = degrees.rem_euclid(360.0);
    // `rem_euclid` of a tiny negative angle rounds up to 360 itself, which a pinned angle is not.
    if wrapped >= 360.0 { 0.0 } else { wrapped }
}
