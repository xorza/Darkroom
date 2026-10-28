//! The tie line: Shift+RMB-drag, or Ctrl+Shift+LMB-drag, across wires ties them into a new tie, or
//! into the first tie it crosses. `darkroom/AGENTS.md`, "Wire ties", "The line only ties", holds
//! the decision. The ties themselves, their hover and press targets and their knobs, are
//! [`tie_widget`].

pub(crate) mod tie_line;
pub(crate) mod tie_menu_ui;
pub(crate) mod tie_probe;
pub(crate) mod tie_widget;

use glam::Vec2;
use palantir::widget::{LineCap, Shape};
use palantir::{Stroke, Ui};

use crate::core::document::graph_view::GraphView;
use crate::core::document::item_id::ItemId;
use crate::core::document::tie::TieId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::route_insert::RouteInsert;
use crate::gui::frame::requests::Requests;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::draw::tie::band::{BandInk, BandLook};
use crate::gui::pane::graph_ui::gesture::button_stroke::{ButtonStroke, StrokeStep};
use crate::gui::pane::graph_ui::gesture::tie_ui::tie_line::{TieLine, WireCrossing};
use crate::gui::pane::graph_ui::gesture::tie_ui::tie_probe::TieProbe;
use crate::gui::pane::graph_ui::prepass_ctx::CanvasGesture;
use crate::gui::theme::Theme;

/// The tie line in flight, as [`BreakerUi`](crate::gui::pane::graph_ui::gesture::breaker_ui::BreakerUi)
/// holds the breaker: the stroke holds the latched button, and the line's crossing buffers
/// outlive the gesture for the next one.
#[derive(Default, Debug)]
pub(crate) struct TieUi {
    /// The stroke in flight. Idle when no line is in flight, which is also what makes
    /// [`Self::line`] meaningless.
    stroke: ButtonStroke,
    line: TieLine,
}

impl TieUi {
    /// Drop the line in flight, keeping its buffers.
    pub(crate) const fn reset(&mut self) {
        self.stroke.reset();
    }

    pub(crate) const fn in_flight(&self) -> bool {
        self.stroke.in_flight()
    }

    /// Drive the line from the outer canvas response: latch it on a tie chord, move its end
    /// with the pointer, and on release tie what the last record pass found it crossing. The
    /// context's Esc drops it.
    pub(crate) fn apply(&mut self, ui: &Ui, ctx: CanvasCtx<'_>, out: &mut Requests) {
        let latch = match ctx.gesture() {
            Some(CanvasGesture::Tie(button)) => Some(button),
            _ => None,
        };
        let zoom = ctx.graph_ctx().viewport().zoom;
        match self.stroke.step(ui, ctx, latch) {
            StrokeStep::Started(at) => self.line.restart(at, zoom),
            StrokeStep::Moved(at) => self.line.reach(at, zoom),
            StrokeStep::Released => release(&self.line, ctx.graph_ctx().view(), out),
            StrokeStep::Idle => {}
        }
    }

    /// Hand the line to the wire and band draws, which find what it crosses, or an inert probe
    /// when no line is in flight. Called once per record pass, which clears last pass's
    /// crossings.
    pub(crate) fn probe(&mut self) -> TieProbe<'_> {
        if !self.stroke.in_flight() {
            return TieProbe::new(None);
        }
        self.line.begin_pass();
        TieProbe::new(Some(&mut self.line))
    }

    /// Paint the line, in the selection colour, over the band it will put where it crosses wires
    /// and no tie: centered on the mean of its crossings, square to the mean of the crossed
    /// strands, a slot for each, as feedforward shows a gesture's result before it ends.
    pub(crate) fn draw(&self, ui: &mut Ui, theme: &Theme, zoom: f32) {
        if !self.stroke.in_flight() {
            return;
        }
        if let Some(preview) = self.line.preview() {
            preview.paint(
                ui,
                theme,
                BandLook {
                    ink: BandInk::Preview,
                    selected: false,
                    pinned: false,
                },
            );
        }
        ui.add_shape(
            Shape::line(
                self.line.start,
                self.line.end,
                Stroke::new(theme.tie.ring, theme.stroke.wire_at(zoom)),
            )
            .cap(LineCap::Round),
        );
    }
}

/// What a released `line` ties, selecting the tie: the wires it crossed join the first tie it
/// crossed, less those the tie holds already, or tie into a new one at the mean of their
/// crossings. A line across nothing ties nothing.
fn release(line: &TieLine, view: &GraphView, out: &mut Requests) {
    let insert = |crossing: &WireCrossing| RouteInsert {
        wire: crossing.wire,
        at: crossing.piece,
    };
    let target = line
        .ties
        .iter()
        .min_by(|a, b| a.along.total_cmp(&b.along))
        .map(|crossing| crossing.tie);
    let tie = match target {
        Some(tie) => {
            let wires: Vec<RouteInsert> = line
                .wires
                .iter()
                .filter(|crossing| {
                    !view
                        .routes
                        .get(&crossing.wire)
                        .is_some_and(|route| route.contains(tie))
                })
                .map(insert)
                .collect();
            if wires.is_empty() {
                return;
            }
            out.push_graph(GraphIntent::JoinTie { tie, wires });
            tie
        }
        None if line.wires.is_empty() => return,
        None => {
            let tie = TieId::unique();
            // The first crossing plus the mean offset from it: a plain sum of the points rounds
            // (three of one x is not exactly 3x), which would set the tie a hair off the line.
            let first = line.wires[0].point;
            let offset: Vec2 = line
                .wires
                .iter()
                .map(|crossing| crossing.point - first)
                .sum();
            out.push_graph(GraphIntent::TieWires {
                tie,
                pos: first + offset / line.wires.len() as f32,
                wires: line.wires.iter().map(insert).collect(),
            });
            tie
        }
    };
    out.push_graph(GraphIntent::SetSelection {
        to: [ItemId::Tie(tie)].into_iter().collect(),
    });
}

#[cfg(test)]
mod tests;
