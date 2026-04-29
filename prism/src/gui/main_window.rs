use std::rc::Rc;

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::graph_ui::GraphUi;
use crate::gui::log_ui::LogUi;
use crate::gui::shortcuts::shortcut_commands;
use crate::gui::style::Style;
use crate::gui::widgets::{Button, ListItem, Modal, Panel, PopupMenu};
use crate::session::Session;
use crate::session::output::{AppCommand, FrameOutput};

use eframe::egui;
use egui::vec2;

#[derive(Debug)]
pub struct MainWindow {
    graph_ui: GraphUi,
    log_ui: LogUi,
    style: Rc<Style>,
    settings_open: bool,
}

impl MainWindow {
    pub fn new() -> Self {
        let style = Style::from_file("style.toml").unwrap_or_default();
        Self {
            graph_ui: GraphUi::default(),
            log_ui: LogUi,
            style: Rc::new(style),
            settings_open: false,
        }
    }

    /// Render one frame, filling `output` with intents/commands the
    /// renderer wants applied. Returns the `AppCommand` (file menu
    /// item, Cmd+Q) if one fired this frame; the caller (`GuiApp`)
    /// applies it on the next `logic` tick. `drain_inbound` /
    /// `handle_output` are *not* called here — `GuiApp::logic` owns
    /// that lifecycle so script side effects flow even when the
    /// window is hidden.
    pub fn render(
        &mut self,
        session: &mut Session,
        output: &mut FrameOutput,
        ui: &mut egui::Ui,
    ) -> Option<AppCommand> {
        let mut gui = Gui::new(ui, &self.style);
        let gui = &mut gui;
        let input = gui.input_snapshot();

        let mut menu_cmd = None;
        Panel::top(StableId::new("top_panel"))
            .show_separator_line(false)
            .show(gui, |gui| {
                gui.horizontal(|gui| {
                    menu_cmd = self.file_menu(gui);
                });
            });

        Panel::bottom(StableId::new("status_panel"))
            .show_separator_line(false)
            .no_frame()
            .show(gui, |gui| {
                self.log_ui.render(gui, session.status());
            });

        Panel::central().no_frame().show(gui, |gui| {
            let render_events = session.take_render_events();
            let ctx = session.graph_context();
            self.graph_ui
                .render(gui, &ctx, render_events, &input, output);
        });

        Modal::new(StableId::new("settings_window"), "Settings")
            .open(&mut self.settings_open)
            .min_size(vec2(300.0, 400.0))
            .show(gui, |_gui| {});

        // Shortcut wins over menu when both fire in the same frame —
        // a Cmd+Q held while the File menu is open should still exit.
        shortcut_commands(&input, session.autorun())
            .apply(output)
            .or(menu_cmd)
    }

    pub(crate) fn handle_app_command(&mut self, session: &mut Session, cmd: AppCommand) {
        match cmd {
            AppCommand::New => session.empty_graph(),
            AppCommand::Save => session.save_graph_dialog(),
            AppCommand::SaveAs => session.save_graph_as_dialog(),
            AppCommand::Open => session.load_graph_dialog(),
            AppCommand::OpenSettings => self.settings_open = true,
            AppCommand::Exit => session.close_app(),
        }
    }

    /// Top-level "File" menu: button that anchors a popup with the
    /// usual New / Save / Save as / Open / Exit entries. No raw egui
    /// chrome — `Button` + `PopupMenu` + `ListItem` carry the whole
    /// thing. `PopupMenu::close_on_click` (default true) closes the
    /// dropdown whenever any entry fires.
    ///
    /// All styling — font, button preset, padding, popup width —
    /// lives on [`MenuStyle`] (`gui.style.menu`). Entries are all
    /// forced to `menu.popup_min_width` so hover highlights the
    /// whole row, not just the text.
    fn file_menu(&mut self, gui: &mut Gui<'_>) -> Option<AppCommand> {
        let menu = gui.style.menu.clone();
        let file_btn = Button::new(StableId::new("menu_file"))
            .text("File")
            .font(menu.font.clone())
            .background(menu.button)
            .padding(menu.padding)
            .show(gui);

        let item_size = vec2(
            menu.popup_min_width,
            gui.font_height(&menu.font) + menu.padding.y * 2.0,
        );
        let entry = |gui: &mut Gui<'_>, id: &'static str, label: &str| -> bool {
            ListItem::from_str(StableId::new(id), label)
                .font(menu.font.clone())
                .style(menu.button)
                .size(item_size)
                .show(gui)
                .clicked()
        };

        PopupMenu::new(&file_btn, StableId::new("menu_file_popup"))
            .min_width(menu.popup_min_width)
            .show(gui, |gui| {
                let mut cmd = None;
                if entry(gui, "menu_file_new", "New") {
                    cmd = Some(AppCommand::New);
                }
                if entry(gui, "menu_file_save", "Save") {
                    cmd = Some(AppCommand::Save);
                }
                if entry(gui, "menu_file_save_as", "Save as") {
                    cmd = Some(AppCommand::SaveAs);
                }
                if entry(gui, "menu_file_open", "Open") {
                    cmd = Some(AppCommand::Open);
                }
                if entry(gui, "menu_file_settings", "Settings...") {
                    cmd = Some(AppCommand::OpenSettings);
                }
                if entry(gui, "menu_file_exit", "Exit") {
                    cmd = Some(AppCommand::Exit);
                }
                cmd
            })
            .flatten()
    }
}
