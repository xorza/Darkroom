//! The Replace dialog: where each port of a node would go in the node put in its place, and the
//! replace it confirms.

use palantir::{
    Button, Configure, Modal, OverlayClose, Panel, Scroll, SizeSpec, Spacing, Text, TextWrap, Ui,
    VAlign, WidgetId, fmt,
};

use crate::core::edit::replace_plan::{PlannedPort, PortSide, ReplacePlan};
use crate::gui::theme::Theme;
use crate::gui::widgets::support;

/// The most the port list shows before it scrolls, in logical px.
const LIST_MAX_HEIGHT: f32 = 320.0;

/// A replace waiting on its dialog.
#[derive(Debug)]
pub(super) struct ReplaceDialog {
    plan: ReplacePlan,
}

/// What a frame of the dialog came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReplaceChoice {
    Stay,
    Cancel,
    Replace,
}

impl ReplaceDialog {
    pub(super) const fn new(plan: ReplacePlan) -> Self {
        Self { plan }
    }

    /// The plan the dialog confirms.
    pub(super) fn into_plan(self) -> ReplacePlan {
        self.plan
    }

    /// Record the dialog over this frame; what its buttons came to.
    pub(super) fn record(&self, ui: &mut Ui, theme: &Theme) -> ReplaceChoice {
        let mut choice = ReplaceChoice::Stay;
        let inset = theme.space.xl;
        let plan = &self.plan;
        let title = ui.fmt(format_args!(
            "Replace \u{201c}{}\u{201d} with {}",
            plan.from_name, plan.to_name
        ));
        let resp = Modal::new()
            .id(WidgetId::from_hash("darkroom.replace.modal"))
            .padding(Spacing::ZERO)
            .show(ui, |ui| {
                Panel::vstack()
                    .id_salt("replace_message")
                    .size(SizeSpec::FILL_W)
                    .padding(Spacing::all(inset))
                    .gap(theme.space.sm)
                    .show(ui, |ui| {
                        Text::new(title)
                            .style(&support::title_text(ui, theme))
                            .text_wrap(TextWrap::Wrap)
                            .show(ui);
                        let note = if plan.ports.is_empty() {
                            "The node saved no ports to move."
                        } else {
                            "Each port's wiring and value move to the port beside it; a port \
                             with none is kept as a ghost, to move by hand."
                        };
                        Text::new(note)
                            .style(&support::muted_text(ui, theme, theme.font.body))
                            .text_wrap(TextWrap::Wrap)
                            .show(ui);
                        Scroll::vertical()
                            .id(WidgetId::from_hash("darkroom.replace.ports"))
                            .size(SizeSpec::HUG)
                            .max_size((f32::INFINITY, LIST_MAX_HEIGHT))
                            .show(ui, |ui| {
                                for (idx, port) in plan.ports.iter().enumerate() {
                                    port_row(ui, theme, idx, port);
                                }
                            });
                    });
                Panel::hstack()
                    .id_salt("replace_footer")
                    .size(SizeSpec::FILL_W)
                    .padding(Spacing::xy(inset, theme.space.lg))
                    .gap(theme.space.sm)
                    .child_align(VAlign::Center)
                    .chrome(theme.footer_chrome(theme.float_inner_radius()))
                    .show(ui, |ui| {
                        support::hspacer(ui, "replace_spacer");
                        if Button::new()
                            .id(WidgetId::from_hash("darkroom.replace.cancel"))
                            .label("Cancel")
                            .show(ui)
                            .left
                            .clicked()
                        {
                            choice = ReplaceChoice::Cancel;
                        }
                        if Button::new()
                            .id(replace_wid())
                            .label("Replace")
                            .style(&theme.primary_button)
                            .show(ui)
                            .left
                            .clicked()
                        {
                            choice = ReplaceChoice::Replace;
                        }
                    });
            });
        if resp.close == Some(OverlayClose::Dismissed) {
            choice = ReplaceChoice::Cancel;
        }
        if choice != ReplaceChoice::Stay {
            ui.withdraw_input_scope(resp.id);
        }
        choice
    }
}

/// One old port's row: its side, its name, and the new port it goes to or "kept as a ghost".
fn port_row(ui: &mut Ui, theme: &Theme, idx: usize, port: &PlannedPort) {
    Panel::hstack()
        .id(WidgetId::from_hash(("darkroom.replace.port", idx)))
        .size(SizeSpec::FILL_W)
        .gap(theme.space.sm)
        .child_align(VAlign::Center)
        .show(ui, |ui| {
            let side = match port.side {
                PortSide::Input => "input",
                PortSide::Output => "output",
                PortSide::Event => "event",
            };
            Text::new(side)
                .style(&support::muted_text(ui, theme, theme.font.caption))
                .show(ui);
            Text::new(port.name.as_str())
                .style(&support::body_text(ui, theme))
                .show(ui);
            let to = match &port.to {
                Some(name) => fmt!(ui, "\u{2192} {name}"),
                None => ui.intern("kept as a ghost"),
            };
            Text::new(to)
                .style(&support::muted_text(ui, theme, theme.font.body))
                .text_wrap(TextWrap::Ellipsis)
                .size(SizeSpec::FILL_W)
                .show(ui);
        });
}

fn replace_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.replace.replace")
}

#[cfg(test)]
mod tests {
    use glam::UVec2;
    use palantir::Harness;
    use scenarium::{CompoundId, GraphId, Node, NodeId, NodeRemap};

    use crate::core::edit::replace_plan::{PlannedPort, PortSide, ReplacePlan};
    use crate::gui::app::replace_dialog::{ReplaceChoice, ReplaceDialog, replace_wid};
    use crate::gui::theme::Theme;

    /// The dialog stays up until a button: Replace reports the replace it confirms.
    #[test]
    fn the_replace_button_confirms_the_plan() {
        let dialog = ReplaceDialog::new(ReplacePlan {
            graph: GraphId::Main,
            node_id: NodeId::from_u128(1),
            with: Node::instance(CompoundId::from_u128(2)),
            remap: NodeRemap::default(),
            from_name: "Denoise".to_owned(),
            to_name: "Median".to_owned(),
            ports: vec![PlannedPort {
                side: PortSide::Input,
                name: "Image".to_owned(),
                to: Some("Image".to_owned()),
            }],
        });
        let theme = Theme::default();
        let mut h = Harness::new(UVec2::new(900, 700));
        let choice = h.frame_value(|ui| dialog.record(ui, &theme));
        assert_eq!(choice, ReplaceChoice::Stay);
        h.prime(1, |ui| {
            dialog.record(ui, &theme);
        });
        h.click(replace_wid());
        let choice = h.frame_value(|ui| dialog.record(ui, &theme));
        assert_eq!(choice, ReplaceChoice::Replace);
    }
}
