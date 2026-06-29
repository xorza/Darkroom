//! The Config tab's content: a settings window for [`AppConfig`] (theme
//! preference + the lens ML model paths). Rendered by `main_window` when
//! the active tab is `TabRef::Config`. Pure view — it reads the current
//! settings from [`AppContext`] and surfaces edits as [`MenuCommand`]s for
//! `App` to apply and persist *outside* the record, exactly like the menu
//! bar (the editor tree doesn't own `AppConfig`).

use std::path::Path;

use palantir::{
    Align, Background, Button, Configure, Corners, Panel, Sizing, Spacing, Text, TextStyle, Ui,
    VAlign,
};

use crate::core::theme_pref::ThemeChoice;
use crate::gui::app::AppContext;
use crate::gui::menu_bar::{MenuCommand, MlModelKind};
use crate::gui::theme::Theme;

/// Draw the config window and return the edit the user requested, if any.
pub(crate) fn show(ui: &mut Ui, ctx: &AppContext<'_>) -> Option<MenuCommand> {
    let theme = ctx.theme;
    let mut command: Option<MenuCommand> = None;
    Panel::vstack()
        .id_salt("config_view")
        .size((Sizing::FILL, Sizing::FILL))
        .padding(Spacing::all(20.0))
        .gap(14.0)
        .background(Background {
            fill: theme.canvas_bg.into(),
            ..Default::default()
        })
        .show(ui, |ui| {
            heading(ui, "Configuration");

            // Appearance — theme preference (mirrors the Theme menu so the
            // setting is reachable from the panel too).
            subheading(ui, theme, "Appearance");
            Panel::hstack()
                .id_salt("config_theme_row")
                .size((Sizing::Hug, Sizing::Hug))
                .gap(6.0)
                .show(ui, |ui| {
                    for (choice, label) in [
                        (ThemeChoice::System, "System"),
                        (ThemeChoice::Dark, "Dark"),
                        (ThemeChoice::Light, "Light"),
                    ] {
                        let mark = if choice == ctx.theme_choice {
                            "● "
                        } else {
                            "○ "
                        };
                        if Button::new()
                            .id_salt(label)
                            .label(format!("{mark}{label}"))
                            .show(ui)
                            .clicked()
                        {
                            command = Some(MenuCommand::SetTheme(choice));
                        }
                    }
                });

            // ML models — caller-supplied ONNX files the ml_denoise /
            // remove_stars nodes load (lumos ships none).
            subheading(ui, theme, "ML Models (ONNX)");
            if let Some(c) = model_row(
                ui,
                theme,
                "Denoise (DeepSNR)",
                &ctx.config.ml_models.denoise,
                MlModelKind::Denoise,
            ) {
                command = Some(c);
            }
            if let Some(c) = model_row(
                ui,
                theme,
                "Star removal (StarNet)",
                &ctx.config.ml_models.star_removal,
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

/// One model-path row: a fixed-width label, the current path in a muted
/// field, and a "Browse…" button that raises a [`MenuCommand::PickMlModel`]
/// for `kind`. Returns the command when the button was clicked.
fn model_row(
    ui: &mut Ui,
    theme: &Theme,
    label: &'static str,
    path: &Path,
    kind: MlModelKind,
) -> Option<MenuCommand> {
    let mut command = None;
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
            Panel::hstack()
                .id_salt("path")
                .size((Sizing::FILL, Sizing::Hug))
                .padding(Spacing::xy(8.0, 4.0))
                .background(Background {
                    fill: theme.header_fill.into(),
                    corners: Corners::all(3.0),
                    ..Default::default()
                })
                .show(ui, |ui| {
                    Text::new(path.display().to_string())
                        .style(TextStyle {
                            color: theme.text_muted,
                            font_size_px: 12.0,
                            ..ui.theme.text
                        })
                        .show(ui);
                });
            if Button::new()
                .id_salt("browse")
                .label("Browse…")
                .show(ui)
                .clicked()
            {
                command = Some(MenuCommand::PickMlModel(kind));
            }
        });
    command
}
