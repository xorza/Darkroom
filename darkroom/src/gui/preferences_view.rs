//! The Preferences tab's content: a settings window for [`Preferences`] (theme
//! preference + the lens ML model paths). Rendered by `main_window` when
//! the active tab is `TabRef::Preferences`. Pure view — it reads the current
//! settings from [`AppContext`] and surfaces edits as [`AppCommand`]s for
//! `App` to apply and persist *outside* the record, exactly like the menu
//! bar (the editor tree doesn't own `Preferences`).

use std::path::{Path, PathBuf};

use palantir::{
    Align, Background, Button, Checkbox, Configure, Panel, RadioButton, Sizing, Spacing, Text,
    TextEdit, TextStyle, Ui, VAlign, WidgetId,
};

use crate::core::theme_pref::ThemeChoice;
use crate::gui::app::AppContext;
use crate::gui::app::{AppCommand, MlModelKind};
use crate::gui::theme::Theme;

/// Draw the preferences window and return the edit the user requested, if any.
pub(crate) fn show(ui: &mut Ui, ctx: &AppContext<'_>) -> Option<AppCommand> {
    let theme = ctx.theme;
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

            // Appearance — the theme preference (the sole home for theme
            // settings now that the Theme menu is gone).
            subheading(ui, theme, "Appearance");
            // The radios mutate a local copy; a change from the current
            // preference becomes a `SetTheme` for `App` to apply + persist.
            let mut selected = ctx.theme_choice;
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
                        RadioButton::new(&mut selected, choice)
                            .label(label)
                            .show(ui);
                    }
                });
            if selected != ctx.theme_choice {
                command = Some(AppCommand::SetTheme(selected));
            }

            // Startup — whether launch reopens the last document.
            subheading(ui, theme, "Startup");
            let mut load_last = ctx.preferences.load_last_document;
            Checkbox::new(&mut load_last)
                .label("Load last document on startup")
                .show(ui);
            if load_last != ctx.preferences.load_last_document {
                command = Some(AppCommand::SetLoadLastDocument(load_last));
            }

            // ML models — caller-supplied ONNX files the ml_denoise /
            // remove_stars nodes load (lumos ships none).
            subheading(ui, theme, "ML Models (ONNX)");
            if let Some(c) = model_row(
                ui,
                "Denoise (DeepSNR)",
                &ctx.preferences.ml_models.denoise,
                MlModelKind::Denoise,
            ) {
                command = Some(c);
            }
            if let Some(c) = model_row(
                ui,
                "Star removal (StarNet)",
                &ctx.preferences.ml_models.star_removal,
                MlModelKind::StarRemoval,
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

/// One model-path row: a fixed-width label, an **editable** path field (type
/// or paste a path; Enter or click-away commits), and a "Browse…" button.
/// Returns a [`AppCommand::SetMlModelPath`] on an edited path or a
/// [`AppCommand::PickMlModel`] when Browse is clicked.
fn model_row(
    ui: &mut Ui,
    label: &'static str,
    path: &Path,
    kind: MlModelKind,
) -> Option<AppCommand> {
    let mut command = None;
    let id = WidgetId::from_hash(("preferences.ml_model_path", label));
    Panel::hstack()
        .id_salt(label)
        .size((Sizing::FILL, Sizing::Hug))
        .gap(8.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            Panel::hstack()
                .id_salt("label")
                .size((Sizing::Fixed(150.0), Sizing::Hug))
                .show(ui, |ui| {
                    Text::new(label)
                        .style(TextStyle {
                            font_size_px: 13.0,
                            ..ui.theme.text
                        })
                        .show(ui);
                });

            // Editable path field. The draft lives across frames in the state
            // map, mirroring the current path while unfocused (so a Browse pick
            // shows up); `TextEdit`'s own submit/blur signals drive the commit —
            // no focus or key polling.
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
                command = Some(AppCommand::SetMlModelPath {
                    kind,
                    path: PathBuf::from(draft),
                });
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
    command
}
