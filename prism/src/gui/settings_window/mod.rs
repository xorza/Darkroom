//! Settings window: editable mirror of the persisted [`Config`] plus
//! the modal that hosts it. Apply writes back to `Session`'s config
//! in memory; `Session::exit` then persists on shutdown. TCP listener
//! changes take effect on next launch — no live restart.
//!
//! Lifecycle: an instance only exists while the modal is visible.
//! Callers hold `Option<SettingsWindow>` — `Some` means open, `None`
//! means closed. [`SettingsWindow::render`] returns `false` when the
//! user dismissed the window this frame; the caller drops the option.

pub mod draft;

pub use draft::SettingsDraft;

use egui::{Align, Layout};

use crate::common::StableId;
use crate::config::Config;
use crate::gui::Gui;
use crate::gui::widgets::{
    Button, Checkbox, Label, Modal, RadioButton, Separator, Space, TextEdit,
};
use crate::session::Session;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsAction {
    Apply,
    Cancel,
}

#[derive(Debug)]
pub struct SettingsWindow {
    draft: SettingsDraft,
    /// Bound to [`Modal::open`] so the close `✕` / Escape can flip
    /// it. Mirrored out as the [`SettingsWindow::render`] return so
    /// the caller can drop the option in the same frame.
    keep_open: bool,
}

impl SettingsWindow {
    /// Snapshot `config` into a fresh editable draft.
    pub fn new(config: &Config) -> Self {
        Self {
            draft: SettingsDraft::from_config(config),
            keep_open: true,
        }
    }

    /// Render one frame. On Apply, calls [`Session::update_config`]
    /// with the validated draft. Returns `false` when the modal was
    /// dismissed this frame (Apply / Cancel / ✕ / Escape) — caller
    /// should drop its `Option<SettingsWindow>`.
    pub fn render(&mut self, gui: &mut Gui<'_>, session: &mut Session) -> bool {
        let mut action: Option<SettingsAction> = None;
        Modal::new(StableId::new("settings_window"), "Settings")
            .open(&mut self.keep_open)
            .show(gui, |gui| {
                action = render_body(gui, &mut self.draft);
            });

        match action {
            Some(SettingsAction::Apply) => {
                if let Ok(new_cfg) = self.draft.to_config() {
                    session.update_config(new_cfg);
                }
                false
            }
            Some(SettingsAction::Cancel) => false,
            None => self.keep_open,
        }
    }
}

fn render_body(gui: &mut Gui<'_>, draft: &mut SettingsDraft) -> Option<SettingsAction> {
    let mut action = None;

    gui.vertical(|gui| {
        gui.form_row(|gui| {
            Checkbox::new(StableId::new("settings_load_last"), &mut draft.load_last)
                .text("Reopen last graph on launch")
                .show(gui);
        });

        gui.form_row(|gui| {
            Checkbox::new(
                StableId::new("settings_tcp_enabled"),
                &mut draft.tcp_enabled,
            )
            .text("Auto-start TCP script listener")
            .show(gui);
        });

        if draft.tcp_enabled {
            gui.indent(StableId::new("settings_tcp_indent"), |gui| {
                render_tcp_section(gui, draft);
            });
        }

        Space::new(gui.style.padding).show(gui);
        Separator::new().show(gui);
        Space::new(gui.style.small_padding).show(gui);

        // -- footer: Cancel / Apply --
        gui.row_with_layout(Layout::right_to_left(Align::Center), |gui| {
            let button_padding = gui.style.button_padding;
            let apply_enabled = draft.is_valid();
            let apply = Button::new(StableId::new("settings_apply"))
                .text("Apply")
                .padding(button_padding)
                .enabled(apply_enabled)
                .show(gui);
            if apply.clicked() && apply_enabled {
                action = Some(SettingsAction::Apply);
            }
            Space::new(gui.style.small_padding).show(gui);
            let cancel = Button::new(StableId::new("settings_cancel"))
                .text("Cancel")
                .padding(button_padding)
                .show(gui);
            if cancel.clicked() {
                action = Some(SettingsAction::Cancel);
            }
        });
    });

    action
}

fn render_tcp_section(gui: &mut Gui<'_>, draft: &mut SettingsDraft) {
    // Bind
    gui.form_row(|gui| {
        Label::new("Bind address").show(gui);
        Space::new(gui.style.padding).show(gui);
        TextEdit::singleline(&mut draft.tcp.bind_text)
            .id(StableId::new("settings_tcp_bind").id())
            .desired_width(200.0)
            .show(gui);
    });
    if let Some(err) = draft.bind_error() {
        gui.form_row(|gui| {
            Label::new(err)
                .color(gui.style.noninteractive_text_color)
                .show(gui);
        });
    }

    // Auth radios — header row, then radios indented under it.
    gui.form_row(|gui| {
        Label::new("Auth").show(gui);
    });
    gui.indent(StableId::new("settings_auth_indent"), |gui| {
        gui.form_row(|gui| {
            RadioButton::new(
                StableId::new("settings_tcp_auth_none"),
                &mut draft.tcp.no_auth,
                true,
            )
            .text("No auth")
            .show(gui);
        });
        gui.form_row(|gui| {
            RadioButton::new(
                StableId::new("settings_tcp_auth_token"),
                &mut draft.tcp.no_auth,
                false,
            )
            .text("Token")
            .show(gui);
        });

        // Token-related rows are only meaningful when auth is on.
        if !draft.tcp.no_auth {
            gui.indent(StableId::new("settings_token_indent"), |gui| {
                gui.form_row(|gui| {
                    Label::new("Token").show(gui);
                    Space::new(gui.style.padding).show(gui);
                    Label::new(draft.tcp.token.to_string())
                        .font(gui.style.mono_font.clone())
                        .truncate(true)
                        .show(gui);
                    Space::new(gui.style.small_padding).show(gui);
                    let regen = Button::new(StableId::new("settings_tcp_token_regen"))
                        .text("Regenerate")
                        .show(gui);
                    if regen.clicked() {
                        draft.regenerate_token();
                    }
                });
                gui.form_row(|gui| {
                    Label::new("Token file").show(gui);
                    Space::new(gui.style.padding).show(gui);
                    TextEdit::singleline(&mut draft.tcp.token_file_text)
                        .id(StableId::new("settings_tcp_token_file").id())
                        .desired_width(220.0)
                        .show(gui);
                    Space::new(gui.style.small_padding).show(gui);
                    let browse = Button::new(StableId::new("settings_tcp_token_file_browse"))
                        .text("Browse")
                        .show(gui);
                    if browse.clicked()
                        && let Some(path) = rfd::FileDialog::new().save_file()
                    {
                        draft.tcp.token_file_text = path.to_string_lossy().to_string();
                    }
                });
            });
        }
    });

    gui.form_row(|gui| {
        Label::new("Takes effect on next launch.")
            .color(gui.style.noninteractive_text_color)
            .show(gui);
    });
}
