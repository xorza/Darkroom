//! The reference tab: every command with its chord, then every gesture of the canvas with its
//! keys, read from the tables the menus and the canvas act by, so the page cannot name a key
//! the editor does not read.

use palantir::{
    Configure, InternedStr, Panel, Scroll, SizeSpec, Spacing, Text, TextStyle, Ui, fmt,
};

use crate::gui::command::Command;
use crate::gui::pane::graph_ui::canvas_input::CanvasGesture;
use crate::gui::theme::Theme;
use crate::gui::widgets::support;

/// Cap on the page's column, so a row's name and its keys stay within one glance on a wide pane.
const COLUMN_MAX_WIDTH: f32 = 480.0;

/// Record the reference tab.
pub(crate) fn show(ui: &mut Ui, theme: &Theme) {
    Scroll::vertical()
        .id_salt("reference")
        .size(SizeSpec::FILL)
        .show(ui, |ui| {
            Panel::vstack()
                .id_salt("reference_body")
                .size(SizeSpec::FILL_W)
                .max_size((COLUMN_MAX_WIDTH, f32::INFINITY))
                .padding(theme.pane_padding())
                .gap(theme.space.lg)
                .show(ui, |ui| {
                    section(ui, theme, "Commands", |ui| {
                        for (idx, command) in Command::ALL.into_iter().enumerate() {
                            let Some(keys) = command.keys() else {
                                continue;
                            };
                            let keys = fmt!(ui, "{keys}");
                            row(ui, theme, idx, command.spec().label, keys);
                        }
                    });
                    section(ui, theme, "On the canvas", |ui| {
                        for (idx, gesture) in CanvasGesture::ALL.into_iter().enumerate() {
                            let hint = gesture.hint();
                            let keys = fmt!(ui, "{}", hint.keys);
                            row(ui, theme, idx, hint.does, keys);
                        }
                    });
                });
        });
}

/// One group of the page: a caption, then its rows.
fn section(ui: &mut Ui, theme: &Theme, caption: &'static str, rows: impl FnOnce(&mut Ui)) {
    Panel::vstack()
        .id_salt(caption)
        .size(SizeSpec::FILL_W)
        .gap(theme.space.xs)
        .show(ui, |ui| {
            Text::new(caption)
                .style(&support::caption_text(ui, theme))
                .show(ui);
            rows(ui);
        });
}

/// One line: what it does at the left, its keys at the right in the figure face.
fn row(ui: &mut Ui, theme: &Theme, idx: usize, does: &'static str, keys: InternedStr) {
    Panel::hstack()
        .id_salt(idx)
        .size(SizeSpec::FILL_W)
        .padding(Spacing::xy(0.0, theme.space.xxs))
        .gap(theme.space.md)
        .show(ui, |ui| {
            Text::new(does)
                .style(&support::body_text(ui, theme))
                .show(ui);
            support::hspacer(ui, "reference_spacer");
            let style = TextStyle {
                color: theme.ink.muted,
                ..support::body_text(ui, theme)
            };
            Text::new(keys).style(&style).show(ui);
        });
}
