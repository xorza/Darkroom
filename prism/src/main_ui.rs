use std::rc::Rc;

use crate::gui::Gui;
use crate::gui::graph_ui::GraphUi;
use crate::gui::graph_ui_interaction::RunCommand;
use crate::gui::log_ui::LogUi;
use crate::gui::style::Style;
use crate::input::InputSnapshot;
use crate::{app_data::AppData, gui::style_settings::StyleSettings};
use eframe::egui;
use egui::{CentralPanel, Frame, Panel, ViewportCommand};

#[derive(Clone, Debug)]
pub struct UiContext {
    ctx: egui::Context,
}

impl UiContext {
    pub fn new(ctx: &egui::Context) -> Self {
        Self { ctx: ctx.clone() }
    }

    pub fn request_redraw(&self) {
        self.ctx.request_repaint();
    }
    pub fn close_app(&self) {
        self.ctx.send_viewport_cmd(ViewportCommand::Close);
    }
}

#[derive(Debug)]
pub struct MainUi {
    pub graph_ui: GraphUi,
    pub log_ui: LogUi,
    pub ui_context: UiContext,
    pub style_settings: Rc<StyleSettings>,

    pub arena: bumpalo::Bump,
}

impl MainUi {
    pub fn new(ctx: &egui::Context) -> Self {
        let style_settings = StyleSettings::from_file("style.toml").unwrap_or_default();

        Self {
            graph_ui: GraphUi::default(),
            log_ui: LogUi,
            ui_context: UiContext::new(ctx),
            style_settings: Rc::new(style_settings),
            arena: bumpalo::Bump::new(),
        }
    }

    pub fn ui_context(&self) -> UiContext {
        self.ui_context.clone()
    }

    fn empty(&mut self, app_data: &mut AppData) {
        self.graph_ui = GraphUi::default();
        app_data.empty_graph();
    }

    fn save(&mut self, app_data: &mut AppData) {
        if let Some(path) = app_data.config.current_path.clone() {
            app_data.save_graph(&path);
        } else {
            self.save_as(app_data);
        }
    }

    fn save_as(&mut self, app_data: &mut AppData) {
        let file = rfd::FileDialog::new()
            .add_filter("Lua", &["lua"])
            .add_filter("Scn", &["scn"])
            .add_filter("JSON", &["json"])
            .add_filter("Lz4 compressed Lua", &["lz4"])
            .save_file();

        if let Some(path) = file {
            app_data.save_graph(&path);
        }
    }

    pub fn load(&mut self, app_data: &mut AppData) {
        let file = rfd::FileDialog::new()
            .add_filter("All supported", &["lua", "json", "scn", "lz4"])
            .add_filter("Lua", &["lua"])
            .add_filter("Scn", &["scn"])
            .add_filter("JSON", &["json"])
            .add_filter("Lz4 compressed Lua", &["lz4"])
            .pick_file();

        if let Some(path) = file {
            self.graph_ui = GraphUi::default();
            app_data.load_graph(&path);
        }
    }

    pub fn render(&mut self, app_data: &mut AppData, root_ui: &mut egui::Ui) {
        let style = Rc::new(Style::new(self.style_settings.clone(), 1.0));
        root_ui.ctx().global_style_mut(|egui_style| {
            style.apply_to_egui(egui_style);
        });

        let input = InputSnapshot::capture(root_ui.ctx());

        app_data.update_shared_status();

        self.handle_shortcuts(&input, app_data);

        Panel::top("top_panel")
            .show_separator_line(false)
            .show_inside(root_ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    style.apply_menu_style(ui);

                    ui.menu_button("File", |ui| {
                        style.apply_menu_style(ui);

                        ui.set_min_width(100.0);
                        if ui.button("New").clicked() {
                            self.empty(app_data);
                            ui.close();
                        }
                        if ui.button("Save").clicked() {
                            self.save(app_data);
                            ui.close();
                        }
                        if ui.button("Save as").clicked() {
                            self.save_as(app_data);
                            ui.close();
                        }
                        if ui.button("Open").clicked() {
                            self.load(app_data);
                            ui.close();
                        }
                        if ui.button("Exit").clicked() {
                            ui.close();
                            self.ui_context.close_app();
                        }
                    });
                });
            });

        Panel::bottom("status_panel")
            .show_separator_line(false)
            .frame(Frame::NONE)
            .show_inside(root_ui, |ui| {
                self.log_ui
                    .render(&mut Gui::new(ui, &style), &app_data.status);
            });

        CentralPanel::default()
            .frame(Frame::NONE)
            .show_inside(root_ui, |ui| {
                self.graph_ui
                    .render(&mut Gui::new(ui, &style), app_data, &input, &self.arena)
            });

        app_data.handle_interaction(self.graph_ui.ui_interaction());
        self.arena.reset();
    }

    fn handle_shortcuts(&mut self, input: &InputSnapshot, app_data: &mut AppData) {
        if input.cmd_only(egui::Key::Z) {
            app_data.undo(self.graph_ui.ui_interaction());
        } else if input.cmd_shift(egui::Key::Z) {
            app_data.redo();
        }

        if input.cmd_shift(egui::Key::S) {
            self.save_as(app_data);
        } else if input.cmd_only(egui::Key::S) {
            self.save(app_data);
        } else if input.cmd(egui::Key::O) {
            self.load(app_data);
        }

        if input.cmd_shift(egui::Key::Space) {
            let interaction = self.graph_ui.ui_interaction();
            interaction.set_run_cmd(if app_data.autorun {
                RunCommand::StopAutorun
            } else {
                RunCommand::StartAutorun
            });
        } else if input.cmd_only(egui::Key::Space) {
            self.graph_ui
                .ui_interaction()
                .set_run_cmd(RunCommand::RunOnce);
        }

        if input.cmd(egui::Key::Q) {
            self.ui_context.close_app();
        }
    }
}
