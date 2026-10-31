//! The ties on the canvas as widgets: each band paints, and records a hover and press target the
//! size of the band and its head; a selected one records its knob too. A hover lights the tie's
//! wires, a press selects it and starts the group drag, and the knob's drag turns the band.

use std::sync::Arc;

use glam::Vec2;
use palantir::{Configure, Panel, Sense, Sizing, Ui};

use crate::core::document::item_id::ItemId;
use crate::core::document::tie::{TieAngle, TieId};
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::click_mode::ClickMode;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::gui::frame::requests::Requests;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::draw::tie::band::{BandAxes, BandInk, BandLook, KNOB_HIT_RADIUS};
use crate::gui::pane::graph_ui::draw_ctx::DrawCtx;
use crate::gui::pane::graph_ui::frame::tie_layout::TieLayout;
use crate::gui::pane::graph_ui::gesture::group_drag::{self, Grab};
use crate::gui::pane::graph_ui::gesture::tie_ui::tie_probe::TieProbe;
use crate::gui::pane::graph_ui::prepass_ctx::PrepassCtx;
use crate::gui::pane::graph_ui::wid;

/// The step a knob turned with Shift rounds to, in degrees, as design tools snap.
const SNAP_DEGREES: f32 = 15.0;

/// The ties' own gesture, the knob drag that turns one; a press on a tie reports a grab, which
/// the canvas's one group drag latches.
#[derive(Default, Debug)]
pub(crate) struct TieWidgets {
    knob: Option<KnobDrag>,
}

/// What recording the ties reports: the tie a right-click landed on, whose menu the caller
/// opens once the draw is over, and the drag a press started.
#[derive(Debug, Default)]
pub(crate) struct TiesRecorded {
    pub(crate) menu_opened: Option<TieId>,
    pub(crate) grab: Option<Grab>,
}

/// A knob held: the tie it turns, the angle to put back on Esc, and the undo gesture its frames
/// fold into.
#[derive(Clone, Copy, Debug)]
struct KnobDrag {
    tie: TieId,
    from: TieAngle,
    gesture: GestureId,
}

/// The canvas pass the ties record in: the draw context, the layout of their bands, the tie line
/// they light under, and the queue their presses raise edits into.
#[derive(Debug)]
pub(crate) struct TiePass<'a, 't> {
    pub(crate) draw_ctx: DrawCtx<'a>,
    pub(crate) ties: &'a TieLayout,
    pub(crate) line: &'a mut TieProbe<'t>,
    pub(crate) out: &'a mut Requests,
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
    pub(crate) fn prepass(&mut self, ui: &mut Ui, ctx: PrepassCtx<'_>, out: &mut Requests) {
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
        let snap = if ui.modifiers().shift {
            AngleSnap::Steps
        } else {
            AngleSnap::Free
        };
        if let Some(pointer) = ui.response_for(canvas::outer_wid()).pointer_local {
            let offset = graph_ctx.viewport().to_world(pointer) - center;
            out.push_graph(turn(TieAngle::Pinned(knob_angle(offset, snap))));
        }
    }

    /// Paint each band with a strand on screen and record its targets: the hover and press
    /// target, and on a selected tie its knob. Runs after the wires and before the node bodies,
    /// so a body covers a band as it covers wires, and wins a press over it.
    pub(crate) fn record(&mut self, ui: &mut Ui, pass: &mut TiePass<'_, '_>) -> TiesRecorded {
        let mut recorded = TiesRecorded::default();
        let draw_ctx = pass.draw_ctx;
        let theme = draw_ctx.theme();
        let click_mode = ClickMode::with_shift(ui.modifiers().shift);
        for band in pass
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
            let ink = if pass.line.cross_band(band.tie, axes.reach()) {
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
            let body = Panel::zstack()
                .id(wid::tie_body(band.tie))
                .pos(hit.min)
                .size((Sizing::fixed(hit.size.w), Sizing::fixed(hit.size.h)))
                .sense(Sense::CLICK | Sense::DRAG)
                .show(ui, |_| {});
            let (clicked, grabbed, menu) = (
                body.response.clicked(),
                body.response.left.drag.started(),
                body.response.right.clicked(),
            );
            if menu {
                recorded.menu_opened.get_or_insert(band.tie);
            }
            if clicked {
                pass.out.extend_graph(GraphIntent::click(
                    click_mode,
                    draw_ctx.graph_ctx().selected(),
                    item,
                ));
            }
            if grabbed && recorded.grab.is_none() {
                recorded.grab = Some(grab(draw_ctx, band.tie, pass.out));
            }
            if selected {
                self.knob(ui, band.tie, band.angle, axes.knob_center(), pass.out);
            }
        }
        recorded
    }

    /// Record the knob of a selected tie: its drag latches the turn, and a double-click frees
    /// the angle.
    fn knob(&mut self, ui: &mut Ui, tie: TieId, angle: TieAngle, center: Vec2, out: &mut Requests) {
        let side = KNOB_HIT_RADIUS * 2.0;
        let knob = Panel::zstack()
            .id(wid::tie_knob(tie))
            .pos(center - Vec2::splat(KNOB_HIT_RADIUS))
            .size(side)
            .sense(Sense::CLICK | Sense::DRAG)
            .show(ui, |_| {});
        if knob.response.left.double_clicked() {
            out.push_graph(GraphIntent::SetTieAngle {
                tie,
                angle: TieAngle::Auto,
                gesture: None,
            });
        }
        if knob.response.left.drag.started() {
            self.knob = Some(KnobDrag {
                tie,
                from: angle,
                gesture: out.open_gesture(),
            });
        }
    }
}

/// The group drag of a pressed tie: with the selection when the tie is in it, else alone,
/// selected, with the ties inside the moving nodes, which are none.
fn grab(draw_ctx: DrawCtx<'_>, tie: TieId, out: &mut Requests) -> Grab {
    let graph_ctx = draw_ctx.graph_ctx();
    let item = ItemId::Tie(tie);
    let members = if draw_ctx.is_selected(item) {
        group_drag::selected_group(draw_ctx)
    } else {
        out.extend_graph(GraphIntent::click(
            ClickMode::Replace,
            graph_ctx.selected(),
            item,
        ));
        Arc::from([DragStart {
            item,
            pos: graph_ctx.view().ties[&tie].pos,
        }])
    };
    Grab {
        item,
        members,
        handle: wid::tie_body(tie),
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
