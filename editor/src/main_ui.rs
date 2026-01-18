use std::rc::Rc;

use crate::gui::Gui;
use crate::gui::graph_ui::GraphUi;
use crate::gui::graph_ui_interaction::RunCommand;
use crate::gui::log_ui::LogUi;
use crate::gui::style::Style;
use crate::{app_data::AppData, gui::style_settings::StyleSettings};
use eframe::egui;
use egui::{CentralPanel, Frame, TopBottomPanel, ViewportCommand};

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
            .add_filter("JSON", &["json"])
            .add_filter("YAML", &["yaml", "yml"])
            .add_filter("Scn compressed binary", &["scn"])
            .save_file();

        if let Some(path) = file {
            app_data.save_graph(&path);
        }
    }

    pub fn load(&mut self, app_data: &mut AppData) {
        let file = rfd::FileDialog::new()
            .add_filter("All supported", &["yaml", "yml", "lua", "json", "scn"])
            .add_filter("Lua", &["lua"])
            .add_filter("JSON", &["json"])
            .add_filter("YAML", &["yaml", "yml"])
            .add_filter("Scn compressed binary", &["scn"])
            .pick_file();

        if let Some(path) = file {
            self.graph_ui = GraphUi::default();
            app_data.load_graph(&path);
        }
    }

    pub fn render(&mut self, app_data: &mut AppData, ctx: &egui::Context) {
        let style = Rc::new(Style::new(self.style_settings.clone(), 1.0));
        ctx.style_mut(|egui_style| {
            style.apply_to_egui(egui_style);
        });

        app_data.update_shared_status();

        self.handle_shortcuts(app_data);

        egui::TopBottomPanel::top("top_panel")
            .show_separator_line(false)
            .show(ctx, |ui| {
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

        TopBottomPanel::bottom("status_panel")
            .show_separator_line(false)
            .frame(Frame::NONE)
            .show(ctx, |ui| {
                self.log_ui
                    .render(&mut Gui::new(ui, &style), &app_data.status);
            });

        CentralPanel::default().frame(Frame::NONE).show(ctx, |ui| {
            self.graph_ui
                .render(&mut Gui::new(ui, &style), app_data, &self.arena)
        });

        app_data.handle_interaction();
        self.arena.reset();
    }

    fn handle_shortcuts(&mut self, app_data: &mut AppData) {
        self.handle_undo_shortcut(app_data);
        self.handle_save_load_shortcuts(app_data);
        self.handle_run_shortcuts(app_data);
        self.handle_quit_shortcuts(app_data);
    }

    fn handle_undo_shortcut(&mut self, app_data: &mut AppData) {
        let undo_pressed = self.ui_context.ctx.input(|input| {
            input.key_pressed(egui::Key::Z) && input.modifiers.command && !input.modifiers.shift
        });
        let redo_pressed = self.ui_context.ctx.input(|input| {
            input.key_pressed(egui::Key::Z) && input.modifiers.command && input.modifiers.shift
        });
        if undo_pressed {
            app_data.undo();
        } else if redo_pressed {
            app_data.redo();
        }
    }

    fn handle_save_load_shortcuts(&mut self, app_data: &mut AppData) {
        let save_as_pressed = self.ui_context.ctx.input(|input| {
            input.key_pressed(egui::Key::S) && input.modifiers.command && input.modifiers.shift
        });
        let save_pressed = self.ui_context.ctx.input(|input| {
            input.key_pressed(egui::Key::S) && input.modifiers.command && !input.modifiers.shift
        });
        let open_pressed = self
            .ui_context
            .ctx
            .input(|input| input.key_pressed(egui::Key::O) && input.modifiers.command);

        if save_as_pressed {
            self.save_as(app_data);
        } else if save_pressed {
            self.save(app_data);
        } else if open_pressed {
            self.load(app_data);
        }
    }

    fn handle_run_shortcuts(&mut self, app_data: &mut AppData) {
        let toggle_autorun_pressed = self.ui_context.ctx.input(|input| {
            input.key_pressed(egui::Key::Space) && input.modifiers.command && input.modifiers.shift
        });
        let run_once_pressed = self.ui_context.ctx.input(|input| {
            input.key_pressed(egui::Key::Space) && input.modifiers.command && !input.modifiers.shift
        });

        if toggle_autorun_pressed {
            if app_data.autorun {
                app_data.interaction.run_cmd = RunCommand::StopAutorun;
            } else {
                app_data.interaction.run_cmd = RunCommand::StartAutorun;
            }
        } else if run_once_pressed {
            app_data.interaction.run_cmd = RunCommand::RunOnce;
        }
    }

    fn handle_quit_shortcuts(&mut self, _app_data: &mut AppData) {
        let quit_pressed = self
            .ui_context
            .ctx
            .input(|input| input.key_pressed(egui::Key::Q) && input.modifiers.command);

        if quit_pressed {
            self.ui_context.close_app();
        }
    }
}
