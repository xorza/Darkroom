//! The Preferences tab's content: a settings form for [`Preferences`]
//! (the behavior toggles, the lens ML model paths), laid out as a
//! centered width-capped column of labeled sections.
//! Rendered by `main_window` when the active tab is `TabRef::Preferences`.
//!
//! It edits the borrowed [`Preferences`] **in place**, and `App` finds the change after the record
//! and persists it, so adding a preference is just another widget here; no new command or handler.
//! The "Browse…" buttons raise [`AppCommand::PickMlModel`] so `App` can open the blocking dialog
//! after authoring has released its borrows.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use common::{ErrorChain, fs, process};
use lens::MlModelPaths;
use palantir::{
    Align, Axis, Button, Checkbox, ColorScheme, ComboBox, Configure, DragValue, Grid, Panel,
    Preference, RadioGroup, RadioLook, SizeSpec, Spacing, Text, TextEdit, TextWrap, Track, Ui,
    VAlign, WidgetId,
};

use crate::core::io::preferences::Preferences;
use crate::core::io::preferences::interface_size::InterfaceSize;
use crate::core::io::preferences::memory_headroom::MemoryHeadroom;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::MlModelKind;
use crate::gui::frame::requests::Requests;
use crate::gui::theme::Theme;
use crate::gui::widgets::link::Link;
use crate::gui::widgets::support;

/// Cap on the settings column, so the form reads as a column on a wide
/// window instead of a handful of controls in the corner of a bare sheet.
const COL_WIDTH: f32 = 720.0;

/// The preferences form for one record pass: the theme it paints with and
/// the sink its Browse buttons raise their picks into.
#[derive(Debug)]
pub(crate) struct PreferencesForm<'a> {
    pub(crate) theme: &'a Theme,
    pub(crate) out: &'a mut Requests,
}

impl PreferencesForm<'_> {
    /// Draw the form, editing `preferences` in place, and queue a Browse dialog for `App` to run.
    pub(crate) fn show(mut self, ui: &mut Ui, preferences: &mut Preferences) {
        let theme = self.theme;
        Panel::vstack()
            .id_salt("preferences_view")
            .size(SizeSpec::FILL)
            .padding(theme.pane_padding())
            .child_align(Align::TOP)
            .show(ui, |ui| {
                // One grid for every section, so a label column, a control column and a button
                // column line up from section to section. A cell spans no column past the
                // control column: palantir widens a hug column a span crosses, the fill one
                // included.
                Grid::new()
                    .id_salt("preferences_form")
                    .size(SizeSpec::FILL_W)
                    .max_size((COL_WIDTH, f32::INFINITY))
                    .cols([Track::HUG, Track::FILL, Track::HUG])
                    .rows(&form_rows(theme))
                    .gap(theme.space.sm)
                    .line_gap(theme.space.xs)
                    .show(ui, |ui| {
                        self.caption(ui, "Appearance", APPEARANCE_ROW);
                        self.scheme_row(ui, &mut preferences.color_scheme);
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
                            Checkbox::new(value)
                                .label(label)
                                .grid_cell((row, 1))
                                .show(ui);
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

                        self.caption(ui, "Library", LIBRARY_ROW);
                        self.library_row(ui, &mut preferences.library_folder);

                        self.caption(ui, "Memory", MEMORY_ROW);
                        self.headroom_row(ui, &mut preferences.memory_headroom);
                    });
            });
    }

    /// The memory left free for everything else: Automatic, or a size in GiB, whose field shows
    /// only while Automatic is off, as a figure the automatic headroom does not use would read as
    /// its size. Turning Automatic off starts the field at [`SET_HEADROOM_GIB`].
    fn headroom_row(&self, ui: &mut Ui, headroom: &mut MemoryHeadroom) {
        let row = MEMORY_ROW + 1;
        let theme = self.theme;
        self.label(ui, "Keep free", row);
        Panel::hstack()
            .id_salt("headroom")
            .grid_cell((row, 1))
            .gap(theme.space.md)
            .child_align(VAlign::Center)
            .show(ui, |ui| {
                let mut auto = *headroom == MemoryHeadroom::Auto;
                if Checkbox::new(&mut auto).label("Automatic").show(ui).changed {
                    *headroom = if auto {
                        MemoryHeadroom::Auto
                    } else {
                        MemoryHeadroom::Gib(SET_HEADROOM_GIB)
                    };
                }
                let MemoryHeadroom::Gib(set) = *headroom else {
                    return;
                };
                // An `i64` draft, the integer a drag value binds; its range holds it in a `u32`.
                let mut gib = i64::from(set);
                let edited = DragValue::new(&mut gib)
                    .id(WidgetId::from_hash("darkroom.preferences.headroom"))
                    .range(1.0..=f64::from(MAX_HEADROOM_GIB))
                    .speed(0.1)
                    .suffix(" GiB")
                    .show(ui);
                if edited.changed {
                    let gib = u32::try_from(gib).expect("the field's range holds it in a u32");
                    *headroom = MemoryHeadroom::Gib(gib);
                }
            });
    }

    /// The library folder: an editable path field, empty for none, and a "Browse…" button that
    /// queues [`AppCommand::PickLibraryFolder`].
    fn library_row(&mut self, ui: &mut Ui, folder: &mut Option<PathBuf>) {
        let row = LIBRARY_ROW + 1;
        self.label(ui, "Folder", row);
        let shown = folder
            .as_deref()
            .map_or_else(Default::default, Path::to_string_lossy);
        let response = TextEdit::buffered(shown)
            .id(WidgetId::from_hash("darkroom.preferences.library_folder"))
            .grid_cell((row, 1))
            .align(VAlign::Center)
            .size(SizeSpec::FILL_W)
            .min_size((ML_PATH_FIELD_MIN, 0.0))
            .placeholder("/path/to/library")
            .show(ui);
        if let Some(text) = response.committed_text {
            *folder = (!text.is_empty()).then(|| PathBuf::from(text));
        }
        if Button::new()
            .id_salt("browse_library")
            .label("Browse…")
            .grid_cell((row, 2))
            .align(VAlign::Center)
            .show(ui)
            .left
            .clicked()
        {
            self.out.push_app(AppCommand::PickLibraryFolder);
        }
    }

    /// The scheme: a label and a segmented track of System, Light and Dark, all three in view and
    /// one click from each other, its pill sliding to a pick, as macOS's Appearance setting shows
    /// them. A pick writes `scheme` in place; the app hands it to palantir, which shows it from
    /// the next frame.
    fn scheme_row(&self, ui: &mut Ui, scheme: &mut Preference<ColorScheme>) {
        let row = APPEARANCE_ROW + 1;
        self.label(ui, "Theme", row);
        RadioGroup::new(scheme)
            .id(scheme_wid())
            .axis(Axis::X)
            .look(RadioLook::Segment(None))
            .grid_cell((row, 1))
            .align(Align::LEFT)
            .show(ui, |ui, options| {
                for pick in SCHEMES {
                    options.option(pick).label(scheme_label(pick)).show(ui);
                }
            });
    }

    /// The interface size: a label in the form's label column and a dropdown of the offered
    /// sizes. A pick writes `size` in place; the app applies it from the next frame.
    fn interface_size_row(&self, ui: &mut Ui, size: &mut InterfaceSize) {
        let row = APPEARANCE_ROW + 2;
        self.label(ui, "Interface size", row);
        ComboBox::labeled(size, InterfaceSize::OFFERED, |size| size.label())
            .id(interface_size_wid())
            .grid_cell((row, 1))
            .align(Align::LEFT)
            .show(ui);
    }

    /// One model-path row: a label, an editable path field (Enter or click-away commits, Escape
    /// drops, a blank commit unsets the model), a "Browse…" button, and beneath the field an error
    /// line when the committed path is broken, then a download hint.
    ///
    /// Writes the path `kind` names in `paths` in place and queues
    /// [`AppCommand::PickMlModel`] when Browse is clicked.
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
        // The field is palantir's buffered `TextEdit`: it shows the stored path while idle,
        // keeps an edit in progress, and hands it over on its commit. The problem is re-statted
        // only when the stored path changes and on a commit, never per frame.
        ui.with_state::<PathField, _>(wid, |ui, field| {
            if field.checked != *path {
                field.check(path.as_deref());
            }
            let problem = field.problem;
            self.label(ui, name, at);
            // Borrowed for a UTF-8 path, every path that round-trips through this field.
            let shown = path
                .as_deref()
                .map_or(Cow::Borrowed(""), Path::to_string_lossy);
            let mut edit = TextEdit::buffered(shown)
                .id(wid)
                .grid_cell((at, 1))
                .align(VAlign::Center)
                .size(SizeSpec::FILL_W)
                .min_size((ML_PATH_FIELD_MIN, 0.0))
                .placeholder("/path/to/model.onnx");
            // A broken committed path tints the field's chrome as an error.
            if problem.is_some() {
                edit = edit.style(&theme.field_error);
            }
            let response = edit.show(ui);
            if let Some(text) = response.committed_text {
                *path = committed_path(text);
            }
            if response.committed {
                // Re-stat on every commit — even an unchanged path: the file may have appeared
                // (or vanished) since the last check, and Enter is the natural retry. The stored
                // path, not the text, which is lossy for a name that is not UTF-8.
                field.check(path.as_deref());
            }

            if Button::new()
                .id_salt(("browse", name))
                .label("Browse…")
                .grid_cell((at, 2))
                .align(VAlign::Center)
                .show(ui)
                .left
                .clicked()
            {
                self.out.push_app(AppCommand::PickMlModel(kind));
            }

            // Under the field and the button: the error, then how to get the file.
            Panel::vstack()
                .id_salt(("notes", name))
                .grid_cell((at + 1, 1))
                .size(SizeSpec::FILL_W)
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
                            .size(SizeSpec::FILL_W)
                            .show(ui);
                    }
                    download_hint(ui, theme, row);
                });
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
            .align(VAlign::Center)
            .show(ui);
    }
}

/// The form's grid rows that start each section, a caption then its rows; a model takes two
/// rows, its field's and its notes'.
const APPEARANCE_ROW: u16 = 0;
/// Past the appearance's caption, its theme row and its size row.
const BEHAVIOR_ROW: u16 = APPEARANCE_ROW + 3;
/// Past the behavior's caption and its three toggles.
const MODELS_ROW: u16 = BEHAVIOR_ROW + 4;
/// Past the models' caption, then each model's field row and notes row.
const LIBRARY_ROW: u16 = MODELS_ROW + 1 + 2 * 2;
/// Past the library's caption and its folder row.
const MEMORY_ROW: u16 = LIBRARY_ROW + 2;
/// The memory's caption and its headroom row.
const ROW_COUNT: usize = MEMORY_ROW as usize + 2;

/// The size a set headroom starts at when Automatic is turned off: Auto's floor, ComfyUI's
/// smallest default, a starting point to edit from.
const SET_HEADROOM_GIB: u32 = 2;

/// The most a set headroom takes: a workstation's memory, past which no machine keeps more free.
const MAX_HEADROOM_GIB: u32 = 256;

/// Every row of the form: a caption and a note hug their text, and a row of controls is a
/// row's height, so a checkbox row and a field row pitch alike.
fn form_rows(theme: &Theme) -> [Track; ROW_COUNT] {
    let mut rows = [Track::fixed(theme.row_height()); ROW_COUNT];
    for caption in [
        APPEARANCE_ROW,
        BEHAVIOR_ROW,
        MODELS_ROW,
        LIBRARY_ROW,
        MEMORY_ROW,
    ] {
        rows[usize::from(caption)] = Track::HUG;
    }
    for notes in [MODELS_ROW + 2, MODELS_ROW + 4] {
        rows[usize::from(notes)] = Track::HUG;
    }
    rows
}

/// One ML-model row's fixed description: its name, which preference a Browse pick writes, and
/// where the file comes from.
#[derive(Clone, Copy, Debug)]
struct ModelRow {
    /// The form's grid row the field sits in; its notes take the next.
    row: u16,
    label: &'static str,
    kind: MlModelKind,
    download_label: &'static str,
    download_url: &'static str,
}

/// Floor of an ML-path field, which fills the leftover column width.
const ML_PATH_FIELD_MIN: f32 = 280.0;

/// The model a commit of `text` sets: none for a blank text, as scenarium reads a blank path as
/// unset, else the path as typed, never trimmed, as a file name may start or end in a space.
fn committed_path(text: String) -> Option<PathBuf> {
    (!text.trim().is_empty()).then(|| PathBuf::from(text))
}

/// What's wrong with a set model path, or `None` when it's healthy. Stats the filesystem, so never
/// call it per frame.
fn path_problem(p: &Path) -> Option<&'static str> {
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

/// Cross-frame state for a [`PreferencesForm::model_row`] path field: the path's health, as the
/// last change of the stored path or the last commit found it.
#[derive(Default, Debug)]
struct PathField {
    /// The path [`Self::problem`] was found for, `None` for an unset model.
    checked: Option<PathBuf>,
    /// Nothing for an unset model: the ML nodes are optional.
    problem: Option<&'static str>,
}

impl PathField {
    /// Stat `path`, when one is set, and keep what is wrong with it.
    fn check(&mut self, path: Option<&Path>) {
        self.problem = path.and_then(path_problem);
        self.checked = path.map(Path::to_path_buf);
    }
}

/// The path field of the model row `label`.
fn path_field_wid(label: &str) -> WidgetId {
    WidgetId::from_hash(("darkroom.preferences.ml_model_path", label))
}

/// The CLI tools ship as a zip of self-contained binaries plus the model
/// weights; only the `.onnx` matters to us, so the guidance is the same for
/// every model.
const DOWNLOAD_HINT: &str = " \u{2014} unzip and point the field above at the .onnx file inside";

/// The schemes the Theme row offers, the system's first, as macOS lists Auto beside Light and
/// Dark and Zed's `theme.mode` takes `system`, `light` or `dark`.
const SCHEMES: [Preference<ColorScheme>; 3] = [
    Preference::System,
    Preference::Fixed(ColorScheme::Light),
    Preference::Fixed(ColorScheme::Dark),
];

/// A scheme as the Theme row lists it.
const fn scheme_label(scheme: Preference<ColorScheme>) -> &'static str {
    match scheme {
        Preference::System => "System",
        Preference::Fixed(ColorScheme::Light) => "Light",
        Preference::Fixed(ColorScheme::Dark) => "Dark",
    }
}

fn scheme_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.preferences.scheme")
}

fn interface_size_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.preferences.interface_size")
}

/// Guidance line under a model row: a link to the row's download URL, then muted unzip
/// instructions.
fn download_hint(ui: &mut Ui, theme: &Theme, row: ModelRow) {
    let ModelRow {
        download_label: link_label,
        download_url: url,
        ..
    } = row;
    let wid = WidgetId::from_hash(("darkroom.preferences.download_hint", link_label));
    Panel::hstack()
        .id_salt("hint")
        .size(SizeSpec::FILL_W)
        .child_align(VAlign::Top)
        .show(ui, |ui| {
            // The destination shows under the pointer, as the text does not spell it out. A missing
            // opener logs and degrades: the link has no error to show.
            if Link::new(wid, link_label)
                .tooltip(url)
                .size(theme.font.caption)
                .show(ui, theme)
            {
                process::open_url(url, move |error| {
                    tracing::warn!("failed to open url {url}: {}", ErrorChain::new(&error));
                });
            }
            Text::new(DOWNLOAD_HINT)
                .style(&support::muted_text(ui, theme, theme.font.caption))
                .text_wrap(TextWrap::Wrap)
                .size(SizeSpec::FILL_W)
                .show(ui);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::UVec2;
    use palantir::{Key, Modifiers};

    use common::{TempDir, TempFile, internals};
    use palantir::Harness;

    #[test]
    fn path_problem_classifies_missing_dir_and_extension() {
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
    /// value and nothing is queued. Enter writes the typed path and queues one change, and the
    /// missing file it names shows its problem; a blank commit unsets the model, which has none.
    #[test]
    fn escape_restores_the_path_and_enter_commits_it() {
        let theme = Theme::new(ColorScheme::Dark);
        let row = ModelRow {
            row: 0,
            label: "Denoise model",
            kind: MlModelKind::Denoise,
            download_label: "download",
            download_url: "https://example.invalid",
        };
        let wid = path_field_wid(row.label);
        let committed = Some(PathBuf::from("/a.onnx"));
        let mut paths = MlModelPaths {
            denoise: committed.clone(),
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
        };
        let type_x = |h: &mut Harness,
                      frame: &mut dyn FnMut(&mut Harness, &mut MlModelPaths),
                      paths: &mut MlModelPaths| {
            let center = h.rect(wid).expect("path field arranged").center();
            h.click(center);
            frame(h, paths);
            h.key(Key::End);
            frame(h, paths);
            h.key(Key::Char('x'));
            frame(h, paths);
            assert_eq!(
                paths.denoise.as_deref(),
                Some(Path::new("/a.onnx")),
                "typing writes nothing"
            );
        };

        frame(&mut h, &mut paths);
        type_x(&mut h, &mut frame, &mut paths);
        h.key(Key::Escape);
        for _ in 0..3 {
            frame(&mut h, &mut paths);
        }
        assert_eq!(paths.denoise, committed, "Escape must not write the draft");

        type_x(&mut h, &mut frame, &mut paths);
        h.key(Key::Enter);
        frame(&mut h, &mut paths);
        assert_eq!(
            paths.denoise.as_deref(),
            Some(Path::new("/a.onnxx")),
            "Enter writes the draft"
        );
        assert_eq!(
            paths.star_removal, None,
            "the row writes only the path its kind names"
        );
        let problem = |h: &Harness| {
            h.ui()
                .state::<PathField>(wid)
                .and_then(|field| field.problem)
        };
        assert_eq!(problem(&h), Some("File not found"));

        // Spaces alone are blank: the commit unsets the model.
        h.hold_modifiers(Modifiers::CTRL);
        h.key(Key::Char('a'));
        h.hold_modifiers(Modifiers::NONE);
        frame(&mut h, &mut paths);
        h.type_text("  ");
        h.key(Key::Enter);
        frame(&mut h, &mut paths);
        assert_eq!(paths.denoise, None, "a blank commit unsets the model");
        assert_eq!(problem(&h), None, "an unset model has no problem");
    }
}
