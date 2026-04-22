use std::rc::Rc;

use crate::app_data::AppData;
use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::frame_output::RunCommand;
use crate::gui::graph_ui::GraphUi;
use crate::gui::log_ui::LogUi;
use crate::gui::style::Style;
use crate::gui::widgets::Panel;
use crate::input::InputSnapshot;
use eframe::egui;
use egui::{Id, UiBuilder, ViewportCommand};

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
    /// Reference `Style` at scale=1.0, loaded from `style.toml` on
    /// startup. Serves as the canonical source for [`Gui::new_root`]
    /// every frame.
    pub style: Rc<Style>,

    pub arena: bumpalo::Bump,
}

impl MainUi {
    pub fn new(ctx: &egui::Context) -> Self {
        let style = Style::from_file("style.toml").unwrap_or_default();
        Self {
            graph_ui: GraphUi::default(),
            log_ui: LogUi,
            ui_context: UiContext::new(ctx),
            style: Rc::new(style),
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
        if let Some(path) = app_data.state.config.current_path.clone() {
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

    pub fn render(&mut self, app_data: &mut AppData, gui: &mut Gui<'_>) {
        let style = gui.style.clone();
        let input = gui.input_snapshot();

        app_data.update_shared_status();

        self.handle_shortcuts(&input, app_data);

        Panel::top(StableId::new("top_panel"))
            .show_separator_line(false)
            .show(gui, |gui| {
                // Anchor a global-scope id for the MenuBar so its
                // internal horizontal layout's widget id doesn't drift
                // with the panel's auto-id counter. MenuBar is raw
                // egui chrome and stays that way for now.
                let ui = gui.ui_raw();
                // id-drift-ok
                let _ = ui.scope_builder(UiBuilder::new().id(Id::new("menu_bar_scope")), |ui| {
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
            });

        Panel::bottom(StableId::new("status_panel"))
            .show_separator_line(false)
            .no_frame()
            .show(gui, |gui| {
                self.log_ui.render(gui, &app_data.state.status);
            });

        Panel::central().no_frame().show(gui, |gui| {
            self.graph_ui.render(gui, app_data, &input, &self.arena)
        });

        app_data.handle_output(self.graph_ui.output());
        self.arena.reset();
    }

    fn handle_shortcuts(&mut self, input: &InputSnapshot, app_data: &mut AppData) {
        if input.cmd_only(egui::Key::Z) {
            self.graph_ui.cancel_gesture();
            app_data.undo(self.graph_ui.output());
        } else if input.cmd_shift(egui::Key::Z) {
            self.graph_ui.cancel_gesture();
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
            let output = self.graph_ui.output();
            output.set_run_cmd(if app_data.state.autorun {
                RunCommand::StopAutorun
            } else {
                RunCommand::StartAutorun
            });
        } else if input.cmd_only(egui::Key::Space) {
            self.graph_ui.output().set_run_cmd(RunCommand::RunOnce);
        }

        if input.cmd(egui::Key::Q) {
            self.ui_context.close_app();
        }
    }
}
