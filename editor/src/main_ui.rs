use crate::app_data::AppData;
use crate::gui::Gui;
use crate::gui::graph_ui::GraphUi;
use crate::gui::style::Style;
use eframe::egui;
use egui::{Frame, Sense};

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
}

#[derive(Debug)]
pub struct MainUi {
    pub graph_ui: GraphUi,
    pub ui_context: UiContext,

    pub arena: bumpalo::Bump,
}

impl MainUi {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            graph_ui: GraphUi::default(),
            ui_context: UiContext::new(ctx),
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
        app_data.save_graph();
    }

    pub fn load(&mut self, app_data: &mut AppData) {
        self.graph_ui = GraphUi::default();
        app_data.load_graph();
    }

    pub fn test_graph(&mut self, app_data: &mut AppData) {
        self.graph_ui = GraphUi::default();
        app_data.load_test_graph();
    }

    pub fn render(&mut self, app_data: &mut AppData, ctx: &egui::Context) {
        app_data.update_status();

        self.handle_undo_shortcut(ctx, app_data);

        let style = Style::new(1.0);
        ctx.style_mut(|egui_style| {
            style.apply(egui_style);
        });

        egui::TopBottomPanel::top("top_panel")
            .show_separator_line(false)
            .show(ctx, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    {
                        let style = ui.style_mut();
                        style.spacing.button_padding = egui::vec2(16.0, 5.0);
                        style.spacing.item_spacing = egui::vec2(10.0, 5.0);
                        style
                            .text_styles
                            .entry(egui::TextStyle::Button)
                            .and_modify(|font| font.size = 18.0);
                    }
                    ui.menu_button("File", |ui| {
                        {
                            let style = ui.style_mut();
                            style.spacing.button_padding = egui::vec2(16.0, 5.0);
                            style.spacing.item_spacing = egui::vec2(10.0, 5.0);
                            style
                                .text_styles
                                .entry(egui::TextStyle::Button)
                                .and_modify(|font| font.size = 18.0);
                        }
                        if ui.button("New").clicked() {
                            self.empty(app_data);
                            ui.close();
                        }
                        if ui.button("Save").clicked() {
                            self.save(app_data);
                            ui.close();
                        }
                        if ui.button("Load").clicked() {
                            self.load(app_data);
                            ui.close();
                        }
                        if ui.button("Test").clicked() {
                            self.test_graph(app_data);
                            ui.close();
                        }
                    });
                });
            });

        egui::TopBottomPanel::bottom("status_panel")
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.label(&app_data.status);
            });

        let interaction = egui::CentralPanel::default()
            .frame(Frame::NONE)
            .show(ctx, |ui| {
                self.graph_ui.render(
                    &mut Gui::new(ui, style),
                    &mut app_data.view_graph,
                    app_data.execution_stats.as_ref(),
                    &app_data.func_lib,
                )
            })
            .inner;

        app_data.handle_graph_ui_actions(interaction);
        self.arena.reset();
    }

    fn handle_undo_shortcut(&mut self, ctx: &egui::Context, app_data: &mut AppData) {
        let undo_pressed = ctx.input(|input| {
            input.key_pressed(egui::Key::Z) && input.modifiers.command && !input.modifiers.shift
        });
        let redo_pressed = ctx.input(|input| {
            input.key_pressed(egui::Key::Z) && input.modifiers.command && input.modifiers.shift
        });
        if undo_pressed {
            app_data.undo();
        } else if redo_pressed {
            app_data.redo();
        }
    }
}
