//! The Preferences tab's content: a settings form for [`Preferences`]
//! (the behavior toggles, the lens ML model paths), laid out as a
//! centered width-capped column of labeled sections.
//! Rendered by `main_window` when the active tab is `TabRef::Preferences`.
//!
//! It edits the borrowed [`Preferences`] **in place** and reports a single
//! [`PreferencesCommand::Changed`] whenever any field moved. `App` synchronizes and
//! persists derived state after authoring, so adding a preference is just
//! another widget here; no new command or handler. The "Browse…" buttons
//! return [`PreferencesCommand::PickMlModel`] so `App` can open the blocking dialog
//! after authoring has released its borrows.

use std::path::{Path, PathBuf};

use common::{fs, process};
use lens::MlModelPaths;
use palantir::{
    Align, Button, Checkbox, ComboBox, Configure, EditEnd, Grid, HAlign, Panel, Sizing, Spacing,
    Text, TextEdit, TextWrap, Track, Ui, VAlign, WidgetId,
};

use crate::core::io::preferences::Preferences;
use crate::core::io::preferences::interface_size::InterfaceSize;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::{MlModelKind, PreferencesCommand};
use crate::gui::frame::requests::Requests;
use crate::gui::theme::Theme;
use crate::gui::widgets::edit_buffer::{EditBuffer, Focus};
use crate::gui::widgets::link::Link;
use crate::gui::widgets::support;

/// Cap on the settings column, so the form reads as a column on a wide
/// window instead of a handful of controls in the corner of a bare sheet.
const COL_WIDTH: f32 = 720.0;

/// The preferences form for one record pass: the theme it paints with and
/// the sink its edits queue into.
#[derive(Debug)]
pub(crate) struct PreferencesForm<'a> {
    pub(crate) theme: &'a Theme,
    pub(crate) out: &'a mut Requests,
}

impl PreferencesForm<'_> {
    /// Draw the form, editing `preferences` in place, and queue whatever the edit
    /// needs `App` to run: persist + re-sync, or a Browse dialog.
    pub(crate) fn show(mut self, ui: &mut Ui, preferences: &mut Preferences) {
        let theme = self.theme;
        Panel::vstack()
            .id_salt("preferences_view")
            .size((Sizing::FILL, Sizing::FILL))
            .padding(theme.pane_padding())
            .child_align(Align::new(HAlign::Center, VAlign::Top))
            .show(ui, |ui| {
                // One grid for every section, so a label column, a control column and a button
                // column line up from section to section. A cell spans no column past the
                // control column: palantir widens a hug column a span crosses, the fill one
                // included.
                Grid::new()
                    .id_salt("preferences_form")
                    .size((Sizing::FILL, Sizing::HUG))
                    .max_size((COL_WIDTH, f32::INFINITY))
                    .cols([Track::HUG, Track::FILL, Track::HUG])
                    .rows(&form_rows(theme))
                    .gap(theme.space.sm)
                    .line_gap(theme.space.xs)
                    .show(ui, |ui| {
                        self.caption(ui, "Appearance", APPEARANCE_ROW);
                        self.interface_size_row(ui, &mut preferences.interface_size);

                        self.caption(ui, "Behavior", BEHAVIOR_ROW);
                        let toggles = [
                            (
                                &mut preferences.load_last_document,
                                "Load last document on startup",
                            ),
                            (
                                &mut preferences.confirm_unsaved_changes,
                                "Ask to save unsaved changes",
                            ),
                            (
                                &mut preferences.relative_paths,
                                "Save paths inside the project folder as relative",
                            ),
                        ];
                        for (row, (value, label)) in (BEHAVIOR_ROW + 1..).zip(toggles) {
                            if Checkbox::new(value)
                                .label(label)
                                .grid_cell((row, 1))
                                .show(ui)
                                .changed
                            {
                                self.out
                                    .push_app(AppCommand::Preferences(PreferencesCommand::Changed));
                            }
                        }

                        // ML models — caller-supplied ONNX files the ml_denoise /
                        // remove_stars nodes load (lumos ships none).
                        self.caption(ui, "ML Models", MODELS_ROW);
                        self.model_row(
                            ui,
                            ModelRow {
                                row: MODELS_ROW + 1,
                                label: "Denoise (DeepSNR)",
                                kind: MlModelKind::Denoise,
                                download_label: "Download DeepSNR CLI \u{2197}",
                                download_url: "https://starnetastro.com/cli-tools/deepsnr/",
                            },
                            &mut preferences.ml_models,
                        );
                        self.model_row(
                            ui,
                            ModelRow {
                                row: MODELS_ROW + 3,
                                label: "Star removal (StarNet)",
                                kind: MlModelKind::StarRemoval,
                                download_label: "Download StarNet CLI \u{2197}",
                                download_url: "https://starnetastro.com/cli-tools/starnet/",
                            },
                            &mut preferences.ml_models,
                        );
                    });
            });
    }

    /// The interface size: a label in the form's label column and a dropdown of the offered
    /// sizes. A pick writes `size` in place and queues [`PreferencesCommand::Changed`]; the app
    /// applies it from the next frame.
    fn interface_size_row(&mut self, ui: &mut Ui, size: &mut InterfaceSize) {
        let row = APPEARANCE_ROW + 1;
        self.label(ui, "Interface size", row);
        let before = size.offered_idx();
        let mut picked = before;
        ComboBox::labeled(&mut picked, &InterfaceSize::OFFERED, |size| size.label())
            .id(interface_size_wid())
            .grid_cell((row, 1))
            .align(Align::new(HAlign::Left, VAlign::Center))
            .show(ui);
        if picked != before {
            *size = InterfaceSize::OFFERED[picked];
            self.out
                .push_app(AppCommand::Preferences(PreferencesCommand::Changed));
        }
    }

    /// One model-path row: a fixed-width label, an **editable** path field (type
    /// or paste a path; Enter or click-away commits it, Escape drops it), a
    /// "Browse…" button, and — beneath, indented under the field — an error line
    /// when the committed path is broken, then a download hint (a browser link to
    /// the CLI tool plus unzip/point-at-the-`.onnx` guidance).
    ///
    /// Writes the path `kind` names in `paths` in place and queues
    /// [`PreferencesCommand::Changed`] on an edited path, or
    /// [`PreferencesCommand::PickMlModel`] when Browse is clicked.
    fn model_row(&mut self, ui: &mut Ui, row: ModelRow, paths: &mut MlModelPaths) {
        let ModelRow {
            row: at,
            label: name,
            kind,
            ..
        } = row;
        let theme = self.theme;
        let path = kind.path_mut(paths);
        let wid = path_field_wid(name);
        // Refresh the draft from `path` only while the field is idle, so an in-progress edit
        // is never overwritten, the blur frame that commits it included: focus is resolved
        // before the record pass, so that frame already reads unfocused
        // ([`EditBuffer::is_idle`]). The problem is re-statted only when the mirrored path
        // changes and on a commit, never per frame.
        ui.with_state::<PathField, _>(wid, |ui, field| {
            if field.buffer.is_idle(Focus::of(ui, wid)) {
                // Borrowed for a UTF-8 path, every path that round-trips through this field, so
                // the common unchanged case compares in place.
                let canonical = path.to_string_lossy();
                if field.buffer.text != canonical {
                    field.buffer.text.replace_range(.., &canonical);
                    field.problem = path_problem(path);
                }
            }
            let problem = field.problem;
            self.label(ui, name, at);
            let mut edit = TextEdit::new(&mut field.buffer.text)
                .id(wid)
                .grid_cell((at, 1))
                .align(Align::v(VAlign::Center))
                .size((Sizing::FILL, Sizing::HUG))
                .min_size((ML_PATH_FIELD_MIN, 0.0))
                .placeholder("/path/to/model.onnx");
            // A broken committed path recolors the field's chrome to the error tint (message
            // under the row says what's wrong).
            if problem.is_some() {
                edit = edit.style(&theme.field_error);
            }
            let response = edit.show(ui);
            if response.ended == Some(EditEnd::Canceled) {
                // Escape drops the draft: the field shows the committed path again.
                field.buffer.text.replace_range(.., &path.to_string_lossy());
            }
            if response.committed {
                if field.buffer.text != path.to_string_lossy() {
                    *path = PathBuf::from(field.buffer.text.clone());
                    self.out
                        .push_app(AppCommand::Preferences(PreferencesCommand::Changed));
                }
                // Re-stat on every commit — even an unchanged path: the file may have appeared
                // (or vanished) since the last check, and Enter is the natural retry. The stored
                // path, not the text, which is lossy for a name that is not UTF-8.
                field.problem = path_problem(path);
            }

            if Button::new()
                .id_salt(("browse", name))
                .label("Browse…")
                .grid_cell((at, 2))
                .align(Align::v(VAlign::Center))
                .show(ui)
                .left
                .clicked()
            {
                self.out
                    .push_app(AppCommand::Preferences(PreferencesCommand::PickMlModel(
                        kind,
                    )));
            }

            // Under the field and the button: the error, then how to get the file.
            Panel::vstack()
                .id_salt(("notes", name))
                .grid_cell((at + 1, 1))
                .size((Sizing::FILL, Sizing::HUG))
                .gap(theme.space.xs)
                .show(ui, |ui| {
                    if let Some(problem) = problem {
                        Text::new(problem)
                            .id_salt("problem")
                            .style(&support::colored_text(
                                ui,
                                theme.status.error,
                                theme.font.caption,
                            ))
                            .text_wrap(TextWrap::Wrap)
                            .size((Sizing::FILL, Sizing::HUG))
                            .show(ui);
                    }
                    download_hint(ui, theme, row);
                });
            field.buffer.settle(Focus::of(ui, wid));
        });
    }

    /// A section's caption in the label column, a section's gap above it past the first, so
    /// proximity, not dividers, makes the groups read.
    fn caption(&self, ui: &mut Ui, title: &'static str, row: u16) {
        let theme = self.theme;
        let above = if row == 0 {
            0.0
        } else {
            theme.space.xl - theme.space.xs
        };
        Text::new(title)
            .id_salt(("caption", title))
            .style(&support::caption_text(ui, theme))
            .grid_cell((row, 0))
            .margin(Spacing::new(0.0, above, 0.0, 0.0))
            .show(ui);
    }

    /// A row's label in the form's label column.
    fn label(&self, ui: &mut Ui, text: &'static str, row: u16) {
        let theme = self.theme;
        Text::new(text)
            .id_salt(("label", text))
            .style(&support::body_text(ui, theme))
            .grid_cell((row, 0))
            .align(Align::v(VAlign::Center))
            .show(ui);
    }
}

/// The form's grid rows that start each section, a caption then its rows; a model takes two
/// rows, its field's and its notes'.
const APPEARANCE_ROW: u16 = 0;
const BEHAVIOR_ROW: u16 = 2;
const MODELS_ROW: u16 = 6;
/// The models' caption, then each model's field row and notes row.
const ROW_COUNT: usize = MODELS_ROW as usize + 1 + 2 * 2;

/// Every row of the form: a caption and a note hug their text, and a row of controls is a
/// row's height, so a checkbox row and a field row pitch alike.
fn form_rows(theme: &Theme) -> [Track; ROW_COUNT] {
    let mut rows = [Track::fixed(theme.row_height()); ROW_COUNT];
    for caption in [APPEARANCE_ROW, BEHAVIOR_ROW, MODELS_ROW] {
        rows[usize::from(caption)] = Track::HUG;
    }
    for notes in [MODELS_ROW + 2, MODELS_ROW + 4] {
        rows[usize::from(notes)] = Track::HUG;
    }
    rows
}

/// One ML-model row's fixed description: what it is called, which preference
/// a Browse pick writes, and where the file comes from. Everything about the
/// row that the form knows before it draws.
#[derive(Clone, Copy, Debug)]
struct ModelRow {
    /// The form's grid row the field sits in; its notes take the next.
    row: u16,
    label: &'static str,
    kind: MlModelKind,
    download_label: &'static str,
    download_url: &'static str,
}

/// Floor of an ML-path field — it fills the leftover column width, but never
/// collapses below a usefully-readable span.
const ML_PATH_FIELD_MIN: f32 = 280.0;

/// What's wrong with a committed model path, or `None` when it's empty (a
/// valid resting state — the ML nodes are optional) or healthy. Stats the
/// filesystem, so callers refresh it only on value changes and commits —
/// never per frame.
fn path_problem(p: &Path) -> Option<&'static str> {
    if p.as_os_str().is_empty() {
        return None;
    }
    if fs::is_dir(p) {
        return Some("Points at a folder \u{2014} pick the .onnx file inside");
    }
    if !fs::is_file(p) {
        return Some("File not found");
    }
    let onnx = p
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("onnx"));
    (!onnx).then_some("Not an .onnx file")
}

/// Cross-frame state for a [`PreferencesForm::model_row`] path field: the buffered text, as a
/// const editor keeps it, and the path's health, cached as the mirror and each commit last
/// found it.
#[derive(Default, Debug)]
struct PathField {
    buffer: EditBuffer,
    problem: Option<&'static str>,
}

/// The path field of the model row `label`.
fn path_field_wid(label: &str) -> WidgetId {
    WidgetId::from_hash(("darkroom.preferences.ml_model_path", label))
}

/// The CLI tools ship as a zip of self-contained binaries plus the model
/// weights; only the `.onnx` matters to us, so the guidance is the same for
/// every model.
const DOWNLOAD_HINT: &str = " \u{2014} unzip and point the field above at the .onnx file inside";

fn interface_size_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.preferences.interface_size")
}

/// Guidance line under a model row: a clickable "Download … CLI ↗" link
/// (accent-colored, brightening on hover; opens the row's download URL in the
/// browser on click) followed by muted instructions to unzip the download and
/// point the field at the `.onnx` inside.
fn download_hint(ui: &mut Ui, theme: &Theme, row: ModelRow) {
    let ModelRow {
        download_label: link_label,
        download_url: url,
        ..
    } = row;
    let wid = WidgetId::from_hash(("darkroom.preferences.download_hint", link_label));
    Panel::hstack()
        .id_salt("hint")
        .size((Sizing::FILL, Sizing::HUG))
        .child_align(Align::v(VAlign::Top))
        .show(ui, |ui| {
            // The destination shows under the pointer, as the text does not spell it out. A missing
            // opener logs and degrades: the link has no error to show.
            if Link::new(wid, link_label)
                .tooltip(url)
                .size(theme.font.caption)
                .show(ui, theme)
                && let Err(error) = process::open_url(url)
            {
                tracing::warn!("failed to open url {url}: {error}");
            }
            Text::new(DOWNLOAD_HINT)
                .style(&support::muted_text(ui, theme, theme.font.caption))
                .text_wrap(TextWrap::Wrap)
                .size((Sizing::FILL, Sizing::HUG))
                .show(ui);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::UVec2;
    use palantir::Key;
    use std::iter;

    use common::{TempDir, TempFile, internals};
    use palantir::Harness;

    #[test]
    fn path_problem_classifies_empty_missing_dir_and_extension() {
        // Empty is the valid resting state (the ML nodes are optional).
        assert_eq!(path_problem(Path::new("")), None);
        // A directory is distinguished from a truly absent path — pointing
        // at the unzipped folder is the mistake the download hint warns of.
        let dir = TempDir::new("darkroom-path-dir");
        assert_eq!(
            path_problem(dir.path()),
            Some("Points at a folder \u{2014} pick the .onnx file inside")
        );
        let missing = TempFile::with_extension("darkroom-path-missing", "onnx");
        assert_eq!(path_problem(missing.path()), Some("File not found"));
        // Real files: a wrong extension flags; `.onnx` passes in any case.
        let wrong = TempFile::with_extension("darkroom-path-wrong", "txt");
        fs::write(wrong.path(), b"x").unwrap();
        assert_eq!(path_problem(wrong.path()), Some("Not an .onnx file"));
        let onnx = TempFile::with_extension("darkroom-path-upper", "ONNX");
        fs::write(onnx.path(), b"x").unwrap();
        assert_eq!(path_problem(onnx.path()), None);
        // A name that is not Unicode is checked as itself, not as its lossy text, which names
        // another file. APFS refuses such a name, so there is nothing to check on it.
        let mut odd = dir.join(internals::non_unicode_name(1));
        odd.set_extension("onnx");
        if fs::write(&odd, b"x").is_ok() {
            assert_eq!(path_problem(&odd), None);
            assert_eq!(
                path_problem(Path::new(&*odd.to_string_lossy())),
                Some("File not found"),
                "the lossy text names no file"
            );
        }
    }

    /// Escape in a path field drops the draft: the path and the field go back to the committed
    /// value and nothing is queued. Enter writes the typed path and queues one change.
    #[test]
    fn escape_restores_the_path_and_enter_commits_it() {
        let theme = Theme::default();
        let row = ModelRow {
            row: 0,
            label: "Denoise model",
            kind: MlModelKind::Denoise,
            download_label: "download",
            download_url: "https://example.invalid",
        };
        let wid = path_field_wid(row.label);
        let mut paths = MlModelPaths {
            denoise: PathBuf::from("/a.onnx"),
            ..MlModelPaths::default()
        };
        let mut h = Harness::builder(UVec2::new(800, 200)).mono().build();
        let mut frame = |h: &mut Harness, paths: &mut MlModelPaths| {
            let mut out = Requests::default();
            h.frame(|ui| {
                PreferencesForm {
                    theme: &theme,
                    out: &mut out,
                }
                .model_row(ui, row, paths);
            });
            iter::from_fn(|| out.pop_app())
                .filter(|command| {
                    matches!(
                        command,
                        AppCommand::Preferences(PreferencesCommand::Changed)
                    )
                })
                .count()
        };
        let type_x = |h: &mut Harness,
                      frame: &mut dyn FnMut(&mut Harness, &mut MlModelPaths) -> usize,
                      paths: &mut MlModelPaths| {
            let center = h.rect(wid).expect("path field arranged").center();
            h.click(center);
            frame(h, paths);
            h.key(Key::End);
            frame(h, paths);
            h.key(Key::Char('x'));
            assert_eq!(frame(h, paths), 0, "typing queues nothing");
        };

        frame(&mut h, &mut paths);
        type_x(&mut h, &mut frame, &mut paths);
        h.key(Key::Escape);
        let mut queued = 0;
        for _ in 0..3 {
            queued += frame(&mut h, &mut paths);
        }
        assert_eq!(queued, 0, "Escape must queue no change");
        assert_eq!(
            paths.denoise,
            PathBuf::from("/a.onnx"),
            "Escape must not write the draft"
        );

        type_x(&mut h, &mut frame, &mut paths);
        h.key(Key::Enter);
        assert_eq!(frame(&mut h, &mut paths), 1, "Enter queues one change");
        assert_eq!(paths.denoise, PathBuf::from("/a.onnxx"));
        assert_eq!(
            paths.star_removal,
            MlModelPaths::default().star_removal,
            "the row writes only the path its kind names"
        );
    }
}
