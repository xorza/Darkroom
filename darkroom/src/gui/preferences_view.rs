//! The Preferences tab's content: a settings form for [`Preferences`]
//! (theme preference, startup + exit toggles, the lens ML model paths),
//! laid out as a centered width-capped column of labeled sections.
//! Rendered by `main_window` when the active tab is `TabRef::Preferences`.
//!
//! It edits the borrowed [`Preferences`] **in place** and reports a single
//! [`PrefsCommand::Changed`] whenever any field moved — `App` re-syncs
//! derived state (theme palette, ML paths) and persists once, outside the
//! record. So adding a preference is just another widget here; no new
//! command or handler. The "Browse…" buttons are the exception: they open a
//! blocking file dialog, so they return [`PrefsCommand::PickMlModel`] for
//! `App` to run after the record.

use std::path::{Path, PathBuf};

use aperture::{
    Align, Background, Button, Checkbox, Color, Configure, FontWeight, HAlign, Panel, RadioButton,
    Sense, Sizing, Spacing, Stroke, Text, TextEdit, TextStyle, Tooltip, Ui, VAlign, WidgetId,
};

use crate::core::io::preferences::Preferences;
use crate::core::theme_pref::ThemeChoice;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::prefs::{MlModelKind, PrefsCommand};
use crate::gui::dialogs;
use crate::gui::theme::Theme;

/// Cap on the settings column, so the form reads as a column on a wide
/// window instead of a handful of controls in the corner of a bare sheet.
const COLUMN_WIDTH: f32 = 720.0;

/// Gap between sections. Deliberately much larger than [`SECTION_GAP`] —
/// proximity, not dividers, is what makes the groups read.
const SECTIONS_GAP: f32 = 26.0;

/// Gap between a section's label and its rows, and between the rows.
const SECTION_GAP: f32 = 8.0;

/// Draw the preferences form, editing `prefs` in place. Returns the
/// command the edit needs `App` to run (persist + re-sync, or a Browse
/// dialog), if any.
pub(crate) fn show(ui: &mut Ui, theme: &Theme, prefs: &mut Preferences) -> Option<AppCommand> {
    let mut command: Option<AppCommand> = None;
    Panel::vstack()
        .id_salt("preferences_view")
        .size((Sizing::FILL, Sizing::FILL))
        .padding(Spacing::new(20.0, 32.0, 20.0, 20.0))
        .child_align(Align::new(HAlign::Center, VAlign::Top))
        .background(Background {
            fill: theme.colors.canvas_bg.into(),
            ..Default::default()
        })
        .show(ui, |ui| {
            Panel::vstack()
                .id_salt("preferences_column")
                .size((Sizing::FILL, Sizing::Hug))
                .max_size((COLUMN_WIDTH, f32::INFINITY))
                .gap(SECTIONS_GAP)
                .show(ui, |ui| {
                    // Appearance — the theme preference. The radios write
                    // `prefs.theme` directly; a move re-resolves the palette.
                    let before = prefs.theme;
                    section(ui, theme, "Appearance", |ui| {
                        Panel::hstack()
                            .id_salt("preferences_theme_row")
                            .size((Sizing::Hug, Sizing::Hug))
                            .gap(16.0)
                            .show(ui, |ui| {
                                for (choice, label) in [
                                    (ThemeChoice::System, "System"),
                                    (ThemeChoice::Dark, "Dark"),
                                    (ThemeChoice::Light, "Light"),
                                ] {
                                    RadioButton::new(&mut prefs.theme, choice)
                                        .label(label)
                                        .show(ui);
                                }
                            });
                    });
                    if prefs.theme != before {
                        command = Some(AppCommand::Prefs(PrefsCommand::Changed));
                    }

                    // Behavior — the startup + exit toggles.
                    section(ui, theme, "Behavior", |ui| {
                        if Checkbox::new(&mut prefs.load_last_document)
                            .label("Load last document on startup")
                            .show(ui)
                            .clicked()
                        {
                            command = Some(AppCommand::Prefs(PrefsCommand::Changed));
                        }
                        if Checkbox::new(&mut prefs.confirm_unsaved_on_exit)
                            .label("Ask to save changes before quitting")
                            .show(ui)
                            .clicked()
                        {
                            command = Some(AppCommand::Prefs(PrefsCommand::Changed));
                        }
                    });

                    // ML models — caller-supplied ONNX files the ml_denoise /
                    // remove_stars nodes load (lumos ships none).
                    section(ui, theme, "ML Models", |ui| {
                        if let Some(c) = model_row(
                            ui,
                            theme,
                            "Denoise (DeepSNR)",
                            &mut prefs.ml_models.denoise,
                            MlModelKind::Denoise,
                            "Download DeepSNR CLI \u{2197}",
                            "https://starnetastro.com/cli-tools/deepsnr/",
                        ) {
                            command = Some(c);
                        }
                        if let Some(c) = model_row(
                            ui,
                            theme,
                            "Star removal (StarNet)",
                            &mut prefs.ml_models.star_removal,
                            MlModelKind::StarRemoval,
                            "Download StarNet CLI \u{2197}",
                            "https://starnetastro.com/cli-tools/starnet/",
                        ) {
                            command = Some(c);
                        }
                    });
                });
        });
    command
}

/// One settings section: a bold muted label with its rows grouped tight
/// beneath it, the whole block spaced well apart from its neighbors
/// ([`SECTIONS_GAP`] between sections vs. [`SECTION_GAP`] inside one).
fn section(ui: &mut Ui, theme: &Theme, title: &'static str, body: impl FnOnce(&mut Ui)) {
    Panel::vstack()
        .id_salt(title)
        .size((Sizing::FILL, Sizing::Hug))
        .gap(SECTION_GAP)
        .show(ui, |ui| {
            Text::new(title)
                .style(TextStyle {
                    color: theme.colors.text_muted,
                    font_size_px: 13.0,
                    weight: FontWeight::Bold,
                    ..ui.theme.text
                })
                .show(ui);
            body(ui);
        });
}

/// Floor of an ML-path field — it fills the leftover column width, but never
/// collapses below a usefully-readable span.
const ML_PATH_FIELD_MIN: f32 = 280.0;

/// Width of a model row's fixed label column.
const ML_LABEL_WIDTH: f32 = 150.0;

/// Gap between the label column and the path field.
const ML_ROW_GAP: f32 = 8.0;

/// What's wrong with a committed model path, or `None` when it's empty (a
/// valid resting state — the ML nodes are optional) or healthy. Stats the
/// filesystem, so callers refresh it only on value changes and commits —
/// never per frame.
fn path_problem(path: &str) -> Option<&'static str> {
    if path.is_empty() {
        return None;
    }
    let p = Path::new(path);
    if p.is_dir() {
        return Some("Points at a folder \u{2014} pick the .onnx file inside");
    }
    if !p.is_file() {
        return Some("File not found");
    }
    let onnx = p
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("onnx"));
    (!onnx).then_some("Not an .onnx file")
}

/// Cross-frame state for a [`model_row`] path field: the editor's live `text`
/// plus `seen`, the external path value last mirrored into it. The buffer is
/// refreshed from the path only when `seen` diverges from it (an external
/// change — load, Browse, or our own commit landing), so an in-progress edit
/// is never overwritten, including on the blur frame where the commit fires.
/// `problem` (the path health) refreshes on the same mirror *and* on every
/// commit — the latter so re-committing an unchanged path re-stats it (the
/// file may have appeared since) — never per frame.
#[derive(Default, Debug)]
struct PathField {
    text: String,
    seen: String,
    problem: Option<&'static str>,
}

/// One model-path row: a fixed-width label, an **editable** path field (type
/// or paste a path; Enter or click-away commits into `path`), a "Browse…"
/// button, and — beneath, indented under the field — an error line when the
/// committed path is broken, then a download hint (a browser link to the CLI
/// tool plus unzip/point-at-the-`.onnx` guidance).
/// Writes `path` in place and returns [`PrefsCommand::Changed`] on an
/// edited path or [`PrefsCommand::PickMlModel`] when Browse is clicked.
fn model_row(
    ui: &mut Ui,
    theme: &Theme,
    label: &'static str,
    path: &mut PathBuf,
    kind: MlModelKind,
    download_label: &'static str,
    download_url: &'static str,
) -> Option<AppCommand> {
    let mut command = None;
    let id = WidgetId::from_hash(("preferences.ml_model_path", label));
    // Refresh the draft from `path` only when `path` changed *externally*
    // (initial load, a Browse pick, or last frame's commit) — never on
    // "unfocused". Focus is resolved before the record pass, so on the blur
    // frame `focused_id()` already reads unfocused; mirroring there would
    // stomp the just-typed text before `TextEdit`'s submit/blur commit could
    // read it (the field would snap back and nothing would save). Keying the
    // refresh on the external value sidesteps that race entirely — and gives
    // `status` its no-stat-per-frame revalidation point.
    let canonical = path.display().to_string();
    let field = ui.state_mut::<PathField>(id);
    if field.seen != canonical {
        field.text.replace_range(.., &canonical);
        field.seen.replace_range(.., &canonical);
        field.problem = path_problem(&canonical);
    }
    let problem = field.problem;
    let mut draft = std::mem::take(&mut field.text);
    Panel::vstack()
        .id_salt(label)
        .size((Sizing::FILL, Sizing::Hug))
        .gap(4.0)
        .show(ui, |ui| {
            Panel::hstack()
                .id_salt("row")
                .size((Sizing::FILL, Sizing::Hug))
                .gap(ML_ROW_GAP)
                .child_align(Align::v(VAlign::Center))
                .show(ui, |ui| {
                    Panel::hstack()
                        .id_salt("label")
                        .size((Sizing::Fixed(ML_LABEL_WIDTH), Sizing::Hug))
                        .show(ui, |ui| {
                            Text::new(label)
                                .style(TextStyle {
                                    font_size_px: 13.0,
                                    ..ui.theme.text
                                })
                                .show(ui);
                        });

                    let mut edit = TextEdit::new(&mut draft)
                        .id(id)
                        .size((Sizing::FILL, Sizing::Hug))
                        .min_size((ML_PATH_FIELD_MIN, 0.0))
                        .placeholder("/path/to/model.onnx");
                    // A broken committed path recolors the field's chrome to
                    // the error tint (message under the row says what's wrong).
                    if problem.is_some() {
                        let mut style = ui.theme.text_edit.clone();
                        for look in [&mut style.normal, &mut style.focused] {
                            if let Some(bg) = look.background.as_mut() {
                                bg.stroke =
                                    Stroke::solid(theme.colors.exec_errored_glow, bg.stroke.width);
                            }
                        }
                        edit = edit.style(style);
                    }
                    let resp = edit.show(ui);
                    let commit = resp.submitted || resp.lost_focus;
                    if commit && draft != canonical {
                        *path = PathBuf::from(draft.clone());
                        command = Some(AppCommand::Prefs(PrefsCommand::Changed));
                    }
                    let field = ui.state_mut::<PathField>(id);
                    field.text = draft;
                    if commit {
                        // Re-stat on every commit — even an unchanged path:
                        // the file may have appeared (or vanished) since the
                        // last check, and Enter is the natural retry. A
                        // *changed* commit re-checks again next frame via
                        // the `seen` mirror once it lands in `canonical`.
                        field.problem = path_problem(&field.text);
                    }

                    if Button::new()
                        .id_salt("browse")
                        .label("Browse…")
                        .show(ui)
                        .clicked()
                    {
                        command = Some(AppCommand::Prefs(PrefsCommand::PickMlModel(kind)));
                    }
                });

            if let Some(problem) = problem {
                indented_line(ui, "problem", |ui| {
                    Text::new(problem)
                        .style(TextStyle {
                            color: theme.colors.exec_errored_glow,
                            font_size_px: 12.0,
                            ..ui.theme.text
                        })
                        .show(ui);
                });
            }
            download_hint(ui, theme, download_label, download_url);
        });
    command
}

/// The CLI tools ship as a zip of self-contained binaries plus the model
/// weights; only the `.onnx` matters to us, so the guidance is the same for
/// every model.
const DOWNLOAD_HINT: &str = " \u{2014} unzip and point the field above at the .onnx file inside";

/// A line under a model row, indented past the label column so it sits
/// under the path field. `salt` keys the panel explicitly: the error line
/// is conditional, and an auto id would re-key the hint line each time it
/// appears (occurrence-counter shift).
fn indented_line(ui: &mut Ui, salt: &'static str, body: impl FnOnce(&mut Ui)) {
    Panel::hstack()
        .id_salt(salt)
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::new(ML_LABEL_WIDTH + ML_ROW_GAP, 0.0, 0.0, 0.0))
        .gap(0.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, body);
}

/// Guidance line under a model row: a clickable "Download … CLI ↗" link
/// (accent-colored, brightening on hover; opens `url` in the browser on
/// click) followed by muted instructions to unzip the download and point the
/// field at the `.onnx` inside.
fn download_hint(ui: &mut Ui, theme: &Theme, link_label: &'static str, url: &'static str) {
    let id = WidgetId::from_hash(("preferences.download_hint", link_label));
    // Last frame's hover drives the brighten — this frame's response isn't
    // known until after `show`.
    let link_color = if ui.response_for(id).hovered {
        theme.colors.badge_subgraph.midpoint(Color::hex(0xffffff))
    } else {
        theme.colors.badge_subgraph
    };
    indented_line(ui, "hint", |ui| {
        let link = Panel::hstack()
            .id(id)
            .size((Sizing::Hug, Sizing::Hug))
            .sense(Sense::CLICK)
            .show(ui, |ui| {
                Text::new(link_label)
                    .style(TextStyle {
                        color: link_color,
                        font_size_px: 12.0,
                        ..ui.theme.text
                    })
                    .show(ui);
            });
        let snapshot = link.response.snapshot();
        if link.response.clicked() {
            dialogs::open_url(url);
        }
        // Surface the destination on hover so the user sees where the
        // link goes before clicking — the URL isn't otherwise visible.
        Tooltip::for_(&snapshot).text(url).show(ui);
        Text::new(DOWNLOAD_HINT)
            .style(TextStyle {
                color: theme.colors.text_muted,
                font_size_px: 12.0,
                ..ui.theme.text
            })
            .show(ui);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_problem_classifies_empty_missing_dir_and_extension() {
        // Empty is the valid resting state (the ML nodes are optional).
        assert_eq!(path_problem(""), None);
        // A directory is distinguished from a truly absent path — pointing
        // at the unzipped folder is the mistake the download hint warns of.
        let dir = std::env::temp_dir();
        assert_eq!(
            path_problem(dir.to_str().unwrap()),
            Some("Points at a folder \u{2014} pick the .onnx file inside")
        );
        let missing = dir.join("darkroom-test-definitely-missing.onnx");
        assert_eq!(
            path_problem(missing.to_str().unwrap()),
            Some("File not found")
        );
        // Real files: a wrong extension flags; `.onnx` passes in any case.
        let wrong = dir.join(format!("darkroom-path-test-{}.txt", std::process::id()));
        std::fs::write(&wrong, b"x").unwrap();
        assert_eq!(
            path_problem(wrong.to_str().unwrap()),
            Some("Not an .onnx file")
        );
        let onnx = dir.join(format!("darkroom-path-test-{}.ONNX", std::process::id()));
        std::fs::write(&onnx, b"x").unwrap();
        assert_eq!(path_problem(onnx.to_str().unwrap()), None);
        std::fs::remove_file(wrong).unwrap();
        std::fs::remove_file(onnx).unwrap();
    }
}
