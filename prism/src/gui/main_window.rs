use std::rc::Rc;

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::frame_output::{AppCommand, EditorCommand, RunCommand};
use crate::gui::graph_ui::GraphUi;
use crate::gui::log_ui::LogUi;
use crate::gui::style::Style;
use crate::gui::widgets::{Button, ListItem, Panel, PopupMenu};
use crate::input::InputSnapshot;
use crate::session::Session;

use eframe::egui;
use egui::vec2;

#[derive(Debug)]
pub struct MainWindow {
    graph_ui: GraphUi,
    log_ui: LogUi,
    style: Rc<Style>,
}

impl MainWindow {
    pub fn new() -> Self {
        let style = Style::from_file("style.toml").unwrap_or_default();
        Self {
            graph_ui: GraphUi::default(),
            log_ui: LogUi,
            style: Rc::new(style),
        }
    }

    pub fn render(&mut self, session: &mut Session, ui: &mut egui::Ui) {
        let mut gui = Gui::new(ui, &self.style);
        let gui = &mut gui;

        // One frame = one FrameOutput lifetime. Clearing here (rather
        // than inside `GraphUi::render`) means menu items and shortcut
        // handlers can write to the buffer at any point in the frame
        // without being clobbered by a later renderer's own clear.
        self.graph_ui.output().clear();

        session.drain_inbound();

        let input = gui.input_snapshot();

        Panel::top(StableId::new("top_panel"))
            .show_separator_line(false)
            .show(gui, |gui| {
                gui.horizontal(|gui| {
                    self.file_menu(gui);
                });
            });

        Panel::bottom(StableId::new("status_panel"))
            .show_separator_line(false)
            .no_frame()
            .show(gui, |gui| {
                self.log_ui.render(gui, session.status());
            });

        Panel::central()
            .no_frame()
            .show(gui, |gui| self.graph_ui.render(gui, session, &input));

        self.handle_shortcuts(&input, session);

        let app_cmd = self.graph_ui.output().app_cmd();
        session.handle_output(self.graph_ui.output());
        if let Some(cmd) = app_cmd {
            self.handle_app_command(session, cmd);
        }
    }

    fn handle_app_command(&mut self, session: &mut Session, cmd: AppCommand) {
        match cmd {
            AppCommand::New => {
                self.graph_ui = GraphUi::default();
                session.empty_graph();
            }
            AppCommand::Save => session.save_graph_dialog(),
            AppCommand::SaveAs => session.save_graph_as_dialog(),
            AppCommand::Open => session.load_graph_dialog(),
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
    fn file_menu(&mut self, gui: &mut Gui<'_>) {
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

        PopupMenu::new(&file_btn, "menu_file_popup")
            .min_width(menu.popup_min_width)
            .show(gui, |gui| {
                let output = self.graph_ui.output();
                if entry(gui, "menu_file_new", "New") {
                    output.set_app_cmd(AppCommand::New);
                }
                if entry(gui, "menu_file_save", "Save") {
                    output.set_app_cmd(AppCommand::Save);
                }
                if entry(gui, "menu_file_save_as", "Save as") {
                    output.set_app_cmd(AppCommand::SaveAs);
                }
                if entry(gui, "menu_file_open", "Open") {
                    output.set_app_cmd(AppCommand::Open);
                }
                if entry(gui, "menu_file_exit", "Exit") {
                    output.set_app_cmd(AppCommand::Exit);
                }
            });
    }

    fn handle_shortcuts(&mut self, input: &InputSnapshot, session: &Session) {
        let autorun = session.autorun();
        let output = self.graph_ui.output();

        if input.cmd_only(egui::Key::Z) {
            output.set_editor_cmd(EditorCommand::Undo);
        } else if input.cmd_shift(egui::Key::Z) {
            output.set_editor_cmd(EditorCommand::Redo);
        }

        if input.cmd_shift(egui::Key::S) {
            output.set_app_cmd(AppCommand::SaveAs);
        } else if input.cmd_only(egui::Key::S) {
            output.set_app_cmd(AppCommand::Save);
        } else if input.cmd(egui::Key::O) {
            output.set_app_cmd(AppCommand::Open);
        }

        if input.cmd_shift(egui::Key::Space) {
            output.set_run_cmd(if autorun {
                RunCommand::StopAutorun
            } else {
                RunCommand::StartAutorun
            });
        } else if input.cmd_only(egui::Key::Space) {
            output.set_run_cmd(RunCommand::RunOnce);
        }

        if input.cmd(egui::Key::Q) {
            output.set_app_cmd(AppCommand::Exit);
        }
    }
}
