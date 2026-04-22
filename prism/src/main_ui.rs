use std::rc::Rc;

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::frame_output::RunCommand;
use crate::gui::graph_ui::GraphUi;
use crate::gui::log_ui::LogUi;
use crate::gui::style::Style;
use crate::gui::widgets::Panel;
use crate::input::InputSnapshot;
use crate::session::Session;
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
    /// startup. Serves as the canonical source for [`Gui::new`] every
    /// frame.
    pub style: Rc<Style>,

    pub arena: bumpalo::Bump,
}

impl MainUi {
    pub fn new(ui_context: UiContext) -> Self {
        let style = Style::from_file("style.toml").unwrap_or_default();
        Self {
            graph_ui: GraphUi::default(),
            log_ui: LogUi,
            ui_context,
            style: Rc::new(style),
            arena: bumpalo::Bump::new(),
        }
    }

    fn empty(&mut self, session: &mut Session) {
        self.graph_ui = GraphUi::default();
        session.empty_graph();
    }

    fn save(&mut self, session: &mut Session) {
        if let Some(path) = session.state.config.current_path.clone() {
            session.save_graph(&path);
        } else {
            self.save_as(session);
        }
    }

    fn save_as(&mut self, session: &mut Session) {
        let file = rfd::FileDialog::new()
            .add_filter("Lua", &["lua"])
            .add_filter("Scn", &["scn"])
            .add_filter("JSON", &["json"])
            .add_filter("Lz4 compressed Lua", &["lz4"])
            .save_file();

        if let Some(path) = file {
            session.save_graph(&path);
        }
    }

    pub fn load(&mut self, session: &mut Session) {
        let file = rfd::FileDialog::new()
            .add_filter("All supported", &["lua", "json", "scn", "lz4"])
            .add_filter("Lua", &["lua"])
            .add_filter("Scn", &["scn"])
            .add_filter("JSON", &["json"])
            .add_filter("Lz4 compressed Lua", &["lz4"])
            .pick_file();

        if let Some(path) = file {
            self.graph_ui = GraphUi::default();
            session.load_graph(&path);
        }
    }

    pub fn render(&mut self, session: &mut Session, gui: &mut Gui<'_>) {
        let style = gui.style.clone();
        let input = gui.input_snapshot();

        session.update_shared_status();

        self.handle_shortcuts(&input, session);

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
                                self.empty(session);
                                ui.close();
                            }
                            if ui.button("Save").clicked() {
                                self.save(session);
                                ui.close();
                            }
                            if ui.button("Save as").clicked() {
                                self.save_as(session);
                                ui.close();
                            }
                            if ui.button("Open").clicked() {
                                self.load(session);
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
                self.log_ui.render(gui, &session.state.status);
            });

        Panel::central().no_frame().show(gui, |gui| {
            self.graph_ui.render(gui, session, &input, &self.arena)
        });

        session.handle_output(self.graph_ui.output());
        self.arena.reset();
    }

    fn handle_shortcuts(&mut self, input: &InputSnapshot, session: &mut Session) {
        if input.cmd_only(egui::Key::Z) {
            self.graph_ui.cancel_gesture();
            session.undo(self.graph_ui.output());
        } else if input.cmd_shift(egui::Key::Z) {
            self.graph_ui.cancel_gesture();
            session.redo();
        }

        if input.cmd_shift(egui::Key::S) {
            self.save_as(session);
        } else if input.cmd_only(egui::Key::S) {
            self.save(session);
        } else if input.cmd(egui::Key::O) {
            self.load(session);
        }

        if input.cmd_shift(egui::Key::Space) {
            let output = self.graph_ui.output();
            output.set_run_cmd(if session.state.autorun {
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
