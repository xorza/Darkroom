//! [`RunStatus`]: the last run's word on a node, as its footer and the inspector's chip draw it.

use palantir::{
    Align, Configure, Panel, Rect, RgbaF32, Sizing, Spinner, Text, TextStyle, Ui, VAlign, fmt,
};

use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;
use crate::gui::widgets::format::elapsed::Elapsed;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::support;

/// The run time's width floor in figures: `Elapsed` writes at most 7, which
/// `every_run_time_fits_the_reserved_width` pins.
///
/// A node is `Hug` above a min width, so anything that changes the footer's measured width moves
/// the node's right edge — and with it the cached intra-node offsets of every output port, which
/// is what wires anchor to. A live timer re-formats every frame, so without a floor a running
/// node twitches its outgoing wires each time the digit count changes (`9.99s` → `10.00s`,
/// `999.9ms` → `1.00s`). In the inspector's chip the same floor keeps the title beside it still.
///
/// A floor rather than a fixed width: every value up to `999999s` fits inside it and measures
/// identically, and the rare longer one still renders rather than clipping.
const RUN_TIME_GLYPHS: u8 = 7;

/// The last run's word on a node, in its status ink: a check and the time when it ran, the
/// spinner and the live time while it runs, a cross and "Failed" when it failed, and a word for
/// the rest. One table of words, so the node and the inspector say one thing.
#[derive(Debug)]
pub(crate) struct RunStatus<'a> {
    status: &'a ExecStatus,
    failure: Failure,
}

/// Whether a failure's "Failed" shows beside its cross.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    Named,
    /// The caller shows the failure's message beside the cross, which says more.
    Explained,
}

impl<'a> RunStatus<'a> {
    pub(crate) const fn new(status: &'a ExecStatus) -> Self {
        Self {
            status,
            failure: Failure::Named,
        }
    }

    /// The caller draws a failure's message beside it, so a failure shows its cross alone.
    pub(crate) const fn failure_explained(mut self) -> Self {
        self.failure = Failure::Explained;
        self
    }

    /// Record the status as one group, its parts `space.xs` apart.
    pub(crate) fn show(self, ui: &mut Ui, theme: &Theme) {
        let Self { status, failure } = self;
        let color = theme.exec_ink(status);
        Panel::hstack()
            .id_salt("run_status")
            .size((Sizing::HUG, Sizing::HUG))
            .gap(theme.space.xs)
            .child_align(Align::v(VAlign::Center))
            .show(ui, |ui| match status {
                ExecStatus::Executed(secs) => {
                    status_icon(ui, theme, Icon::Check, color);
                    run_time(ui, theme, *secs, color);
                }
                ExecStatus::Running(at) => {
                    Spinner::new()
                        .diameter(theme.icon.inline.icon)
                        .color(color)
                        .show(ui);
                    run_time(ui, theme, at.elapsed().as_secs_f64(), color);
                }
                ExecStatus::Errored(_) => {
                    status_icon(ui, theme, Icon::Cross, color);
                    if failure == Failure::Named {
                        word(ui, theme, "Failed", color);
                    }
                }
                ExecStatus::None => word(ui, theme, "Not run", color),
                ExecStatus::Cached => word(ui, theme, "Cached", color),
                ExecStatus::MissingInputs(_) => word(ui, theme, "Missing inputs", color),
                ExecStatus::Skipped => word(ui, theme, "Skipped \u{b7} input failed", color),
            });
    }
}

/// `icon` at the inline icon size, in `color`.
fn status_icon(ui: &mut Ui, theme: &Theme, icon: Icon, color: RgbaF32) {
    let side = theme.icon.inline.icon;
    Panel::zstack()
        .id_salt("run_status_icon")
        .size(side)
        .show(ui, |ui| {
            icon.draw(ui, Rect::new(0.0, 0.0, side, side), color);
        });
}

/// The run time's style: the figure face, in the status ink.
fn run_time_style(ui: &Ui, theme: &Theme, color: RgbaF32) -> TextStyle {
    TextStyle {
        color,
        ..support::figure_text(ui, theme)
    }
}

fn run_time(ui: &mut Ui, theme: &Theme, secs: f64, color: RgbaF32) {
    let elapsed = fmt!(ui, "{}", Elapsed::new(secs));
    let style = run_time_style(ui, theme, color);
    let floor = support::figure_width(ui, &style, RUN_TIME_GLYPHS);
    Text::new(elapsed)
        .style(&style)
        .min_size((floor, 0.0))
        .show(ui);
}

fn word(ui: &mut Ui, theme: &Theme, text: &'static str, color: RgbaF32) {
    let style = support::colored_text(ui, color, theme.font.caption);
    Text::new(text).style(&style).show(ui);
}

#[cfg(test)]
mod tests {
    use glam::UVec2;
    use palantir::Harness;
    use palantir::WidgetId;

    use super::*;

    /// Every run time `Elapsed::new` writes, from each unit's smallest to its
    /// widest, fits the floor the label reserves — measured with real shaping
    /// in the label's own style, so neither side can drift alone.
    #[test]
    fn every_run_time_fits_the_reserved_width() {
        let secs = [
            0.0, 999.4e-6, 999.94e-3, 9.994, 99.999, 999.994, 999.996, 9_999.96, 999_999.0,
        ];
        let theme = Theme::default();
        let mut h = Harness::new(UVec2::new(800, 200));
        let id = |i: usize| WidgetId::from_hash(("run_time", i));
        let mut floor = 0.0;
        h.frame(|ui| {
            floor = support::figure_width(
                ui,
                &run_time_style(ui, &theme, RgbaF32::WHITE),
                RUN_TIME_GLYPHS,
            );
            for (i, &s) in secs.iter().enumerate() {
                let style = run_time_style(ui, &theme, RgbaF32::WHITE);
                Text::new(Elapsed::new(s).to_string())
                    .id(id(i))
                    .style(&style)
                    .show(ui);
            }
        });
        // Seven figures of the mono face, whose advance is 600 of its 1000 units per em, at the
        // 12 px caption tier: 7 × 7.2 = 50.4 px, up to a whole 51.
        assert_eq!(floor, 51.0);
        let mut widest = 0.0_f32;
        for (i, &s) in secs.iter().enumerate() {
            let width = h.rect(id(i)).expect("label arranged").size.w;
            assert!(
                width <= floor,
                "{} is {width} px wide, past the {floor} px floor",
                Elapsed::new(s),
            );
            widest = widest.max(width);
        }
        assert_eq!(widest, floor, "the widest run time fills the floor");
    }
}
