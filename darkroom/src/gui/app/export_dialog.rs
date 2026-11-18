//! The export dialog: each path the exported definitions hold, each bundled or stripped, and
//! the export it asks for.

use std::collections::BTreeSet;

use palantir::{
    Button, Checkbox, Configure, Modal, OverlayClose, Panel, Scroll, SizeSpec, Spacing, Text,
    TextWrap, Ui, VAlign, WidgetId, fmt,
};
use scenarium::{CompoundId, Library};

use crate::core::document::Document;
use crate::core::io::export::{ExportPath, export_paths};
use crate::gui::theme::Theme;
use crate::gui::widgets::format::bytes::Bytes;
use crate::gui::widgets::support;

/// The most a row list shows before it scrolls, in logical px.
const LIST_MAX_HEIGHT: f32 = 320.0;

/// An export waiting on its dialog: the compound, the paths it holds, and which the person picked
/// to bundle, none at first.
#[derive(Debug)]
pub(super) struct ExportDialog {
    compound: CompoundId,
    name: String,
    paths: Vec<ExportPath>,
    bundled: Vec<bool>,
}

/// What a frame of the dialog came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExportChoice {
    Stay,
    Cancel,
    Export,
}

impl ExportDialog {
    /// The dialog for exporting `document`'s compound `compound`, reading the size of each path
    /// it holds once, here.
    pub(super) fn new(document: &Document, compound: CompoundId, library: &Library) -> Self {
        let paths = export_paths(document, compound, library);
        Self {
            compound,
            name: document
                .graphs
                .compound(compound)
                .expect("an exported compound is held")
                .name
                .clone(),
            bundled: vec![false; paths.len()],
            paths,
        }
    }

    pub(super) const fn compound(&self) -> CompoundId {
        self.compound
    }

    /// The input paths picked to bundle.
    pub(super) fn bundled(&self) -> BTreeSet<String> {
        self.paths
            .iter()
            .zip(&self.bundled)
            .filter(|(path, picked)| **picked && !path.output)
            .map(|(path, _)| path.path.clone())
            .collect()
    }

    /// Record the dialog over this frame; what its buttons came to.
    pub(super) fn record(&mut self, ui: &mut Ui, theme: &Theme) -> ExportChoice {
        let mut choice = ExportChoice::Stay;
        let inset = theme.space.xl;
        let title = ui.fmt(format_args!("Export \u{201c}{}\u{201d}", self.name));
        let resp = Modal::new()
            .id(WidgetId::from_hash("darkroom.export.modal"))
            .padding(Spacing::ZERO)
            .show(ui, |ui| {
                Panel::vstack()
                    .id_salt("export_message")
                    .size(SizeSpec::FILL_W)
                    .padding(Spacing::all(inset))
                    .gap(theme.space.sm)
                    .show(ui, |ui| {
                        Text::new(title)
                            .style(&support::title_text(ui, theme))
                            .text_wrap(TextWrap::Wrap)
                            .show(ui);
                        let note = if self.paths.is_empty() {
                            "It holds no path."
                        } else {
                            "A path left unticked is stripped, and the receiver's default \
                             takes its place; a ticked one travels in the file."
                        };
                        Text::new(note)
                            .style(&support::muted_text(ui, theme, theme.font.body))
                            .text_wrap(TextWrap::Wrap)
                            .show(ui);
                        Scroll::vertical()
                            .id(WidgetId::from_hash("darkroom.export.paths"))
                            .size(SizeSpec::HUG)
                            .max_size((f32::INFINITY, LIST_MAX_HEIGHT))
                            .show(ui, |ui| {
                                for (idx, (path, picked)) in
                                    self.paths.iter().zip(&mut self.bundled).enumerate()
                                {
                                    path_row(ui, theme, idx, path, picked);
                                }
                            });
                    });
                Panel::hstack()
                    .id_salt("export_footer")
                    .size(SizeSpec::FILL_W)
                    .padding(Spacing::xy(inset, theme.space.lg))
                    .gap(theme.space.sm)
                    .child_align(VAlign::Center)
                    .chrome(theme.footer_chrome(theme.float_inner_radius()))
                    .show(ui, |ui| {
                        support::hspacer(ui, "export_spacer");
                        if Button::new()
                            .id(WidgetId::from_hash("darkroom.export.cancel"))
                            .label("Cancel")
                            .show(ui)
                            .left
                            .clicked()
                        {
                            choice = ExportChoice::Cancel;
                        }
                        if Button::new()
                            .id(export_wid())
                            .label("Export\u{2026}")
                            .style(&theme.primary_button)
                            .show(ui)
                            .left
                            .clicked()
                        {
                            choice = ExportChoice::Export;
                        }
                    });
            });
        if resp.close == Some(OverlayClose::Dismissed) {
            choice = ExportChoice::Cancel;
        }
        if choice != ExportChoice::Stay {
            ui.withdraw_input_scope(resp.id);
        }
        choice
    }
}

/// One path's row: the tick that bundles it, off for an output, the path, and its size.
fn path_row(ui: &mut Ui, theme: &Theme, idx: usize, path: &ExportPath, picked: &mut bool) {
    Panel::hstack()
        .id(WidgetId::from_hash(("darkroom.export.path", idx)))
        .size(SizeSpec::FILL_W)
        .gap(theme.space.sm)
        .child_align(VAlign::Center)
        .show(ui, |ui| {
            if path.output {
                Text::new("output, stripped")
                    .style(&support::muted_text(ui, theme, theme.font.caption))
                    .show(ui);
            } else {
                Checkbox::new(picked)
                    .id(bundle_wid(idx))
                    .label("bundle")
                    .show(ui);
            }
            Text::new(path.path.as_str())
                .style(&support::body_text(ui, theme))
                .text_wrap(TextWrap::EllipsisMiddle)
                .size(SizeSpec::FILL_W)
                .show(ui);
            let size = match path.size {
                Some(bytes) => fmt!(ui, "{}", Bytes::new(bytes)),
                None => ui.intern("missing"),
            };
            Text::new(size)
                .style(&support::muted_text(ui, theme, theme.font.caption))
                .show(ui);
        });
}

fn export_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.export.export")
}

/// The bundle tick of the `idx`th path.
fn bundle_wid(idx: usize) -> WidgetId {
    WidgetId::from_hash(("darkroom.export.bundle", idx))
}

#[cfg(test)]
mod tests {
    use glam::UVec2;
    use palantir::ColorScheme;
    use palantir::Harness;

    use crate::core::io::export::ExportPath;
    use crate::gui::app::export_dialog::{ExportChoice, ExportDialog, bundle_wid, export_wid};
    use crate::gui::theme::Theme;
    use scenarium::CompoundId;

    /// Each input path starts stripped; a tick bundles it, an output offers no tick, and Export
    /// reports the paths ticked.
    #[test]
    fn a_ticked_path_is_bundled_and_the_rest_stripped() {
        let path = |path: &str, output| ExportPath {
            path: path.to_string(),
            size: Some(4),
            output,
        };
        let mut dialog = ExportDialog {
            compound: CompoundId::from_u128(1),
            name: "Calibrate".to_string(),
            paths: vec![
                path("/a.fits", false),
                path("/b.fits", false),
                path("/out", true),
            ],
            bundled: vec![false; 3],
        };
        let theme = Theme::new(ColorScheme::Dark);
        let mut h = Harness::new(UVec2::new(900, 700));
        h.prime(2, |ui| {
            dialog.record(ui, &theme);
        });
        assert!(dialog.bundled().is_empty(), "stripped by default");
        assert!(h.rect(bundle_wid(2)).is_none(), "an output offers no tick");
        h.click(bundle_wid(1));
        h.frame(|ui| {
            dialog.record(ui, &theme);
        });
        h.click(export_wid());
        let choice = h.frame_value(|ui| dialog.record(ui, &theme));
        assert_eq!(choice, ExportChoice::Export);
        assert_eq!(
            dialog.bundled().into_iter().collect::<Vec<_>>(),
            ["/b.fits"]
        );
    }
}
