//! The Preferences tab's content: a settings window for [`Preferences`]
//! (theme preference, startup + exit toggles, the lens ML model paths).
//! Rendered by `main_window` when the active tab is `TabRef::Preferences`.
//!
//! It edits the borrowed [`Preferences`] **in place** and reports a single
//! [`AppCommand::PreferencesChanged`] whenever any field moved — `App`
//! re-syncs derived state (theme palette, ML paths) and persists once,
//! outside the record. So adding a preference is just another widget here;
//! no new command or handler. The "Browse…" buttons are the exception:
//! they open a blocking file dialog, so they return
//! [`AppCommand::PickMlModel`] for `App` to run after the record.

use std::path::PathBuf;

use palantir::{
    Align, Background, Button, Checkbox, Color, Configure, Panel, RadioButton, Sense, Sizing,
    Spacing, Text, TextEdit, TextStyle, Ui, VAlign, WidgetId,
};

use crate::core::io::preferences::Preferences;
use crate::core::theme_pref::ThemeChoice;
use crate::gui::app::{AppCommand, MlModelKind};
use crate::gui::dialogs;
use crate::gui::theme::Theme;

/// Draw the preferences window, editing `prefs` in place. Returns the
/// command the edit needs `App` to run (persist + re-sync, or a Browse
/// dialog), if any.
pub(crate) fn show(ui: &mut Ui, theme: &Theme, prefs: &mut Preferences) -> Option<AppCommand> {
    let mut command: Option<AppCommand> = None;
    Panel::vstack()
        .id_salt("preferences_view")
        .size((Sizing::FILL, Sizing::FILL))
        .padding(Spacing::all(20.0))
        .gap(14.0)
        .background(Background {
            fill: theme.canvas_bg.into(),
            ..Default::default()
        })
        .show(ui, |ui| {
            heading(ui, "Preferences");

            // Appearance — the theme preference. The radios write
            // `prefs.theme` directly; a move re-resolves the palette.
            subheading(ui, theme, "Appearance");
            let before = prefs.theme;
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
            if prefs.theme != before {
                command = Some(AppCommand::PreferencesChanged);
            }

            // Startup — whether launch reopens the last document.
            subheading(ui, theme, "Startup");
            if Checkbox::new(&mut prefs.load_last_document)
                .label("Load last document on startup")
                .show(ui)
                .clicked()
            {
                command = Some(AppCommand::PreferencesChanged);
            }

            // Quitting — whether unsaved changes prompt before exit.
            subheading(ui, theme, "Quitting");
            if Checkbox::new(&mut prefs.confirm_unsaved_on_exit)
                .label("Ask to save changes before quitting")
                .show(ui)
                .clicked()
            {
                command = Some(AppCommand::PreferencesChanged);
            }

            // ML models — caller-supplied ONNX files the ml_denoise /
            // remove_stars nodes load (lumos ships none).
            subheading(ui, theme, "ML Models (ONNX)");
            if let Some(c) = model_row(
                ui,
                theme,
                "Denoise (DeepSNR)",
                &mut prefs.ml_models.denoise,
                MlModelKind::Denoise,
                "Download DeepSNR \u{2197}",
                "https://www.deepsnrastro.com/download/",
            ) {
                command = Some(c);
            }
            if let Some(c) = model_row(
                ui,
                theme,
                "Star removal (StarNet)",
                &mut prefs.ml_models.star_removal,
                MlModelKind::StarRemoval,
                "Download StarNet++ \u{2197}",
                "https://www.starnetastro.com/download/",
            ) {
                command = Some(c);
            }
        });
    command
}

/// The window's title text.
fn heading(ui: &mut Ui, text: &'static str) {
    Text::new(text)
        .style(TextStyle {
            font_size_px: 18.0,
            ..ui.theme.text
        })
        .show(ui);
}

/// A muted section label above a group of rows.
fn subheading(ui: &mut Ui, theme: &Theme, text: &'static str) {
    Text::new(text)
        .style(TextStyle {
            color: theme.text_muted,
            font_size_px: 13.0,
            ..ui.theme.text
        })
        .show(ui);
}

/// Width of an ML-path field — wide enough for a deep path, bounded so it
/// doesn't span the whole settings panel.
const ML_PATH_FIELD_WIDTH: f32 = 520.0;

/// Width of a model row's fixed label column.
const ML_LABEL_WIDTH: f32 = 150.0;

/// Gap between the label column and the path field.
const ML_ROW_GAP: f32 = 8.0;

/// One model-path row: a fixed-width label, an **editable** path field (type
/// or paste a path; Enter or click-away commits into `path`), a "Browse…"
/// button, and — beneath, indented under the field — a download link for the
/// backing tool (opens the browser directly on click). Writes `path` in place
/// and returns [`AppCommand::PreferencesChanged`] on an edited path or
/// [`AppCommand::PickMlModel`] when Browse is clicked.
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

                    // Editable path field. The draft lives across frames in the
                    // state map, mirroring the current path while unfocused (so a
                    // Browse pick shows up); `TextEdit`'s own submit/blur signals
                    // drive the commit — no focus or key polling.
                    let canonical = path.display().to_string();
                    if ui.focused_id() != Some(id) {
                        let buf = ui.state_mut::<String>(id);
                        if *buf != canonical {
                            buf.replace_range(.., &canonical);
                        }
                    }
                    let mut draft = std::mem::take(ui.state_mut::<String>(id));
                    let resp = TextEdit::new(&mut draft)
                        .id(id)
                        .size((Sizing::Fixed(ML_PATH_FIELD_WIDTH), Sizing::Hug))
                        .show(ui);
                    let commit = resp.submitted || resp.lost_focus;
                    ui.state_mut::<String>(id).replace_range(.., &draft);
                    if commit && draft != canonical {
                        *path = PathBuf::from(draft);
                        command = Some(AppCommand::PreferencesChanged);
                    }

                    if Button::new()
                        .id_salt("browse")
                        .label("Browse…")
                        .show(ui)
                        .clicked()
                    {
                        command = Some(AppCommand::PickMlModel(kind));
                    }
                });

            download_link(ui, theme, download_label, download_url);
        });
    command
}

/// A "Download …" link under a model row, indented past the label column to
/// sit under the path field. Accent-colored, brightening on hover; opens
/// `url` in the browser on click.
fn download_link(ui: &mut Ui, theme: &Theme, label: &'static str, url: &'static str) {
    let id = WidgetId::from_hash(("preferences.download_link", label));
    // Last frame's hover drives the brighten — this frame's response isn't
    // known until after `show`.
    let color = if ui.response_for(id).hovered {
        theme.badge_subgraph.midpoint(Color::hex(0xffffff))
    } else {
        theme.badge_subgraph
    };
    let clicked = Panel::hstack()
        .id(id)
        .size((Sizing::Hug, Sizing::Hug))
        .sense(Sense::CLICK)
        .padding(Spacing::new(ML_LABEL_WIDTH + ML_ROW_GAP, 0.0, 0.0, 0.0))
        .show(ui, |ui| {
            Text::new(label)
                .style(TextStyle {
                    color,
                    font_size_px: 12.0,
                    ..ui.theme.text
                })
                .show(ui);
        })
        .clicked();
    if clicked {
        dialogs::open_url(url);
    }
}
