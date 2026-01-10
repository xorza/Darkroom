use crate::app_data::AppData;
use crate::gui::Gui;
use crate::gui::graph_ui::{GraphUi, GraphUiInteraction};
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
    pub graph_ui_interaction: GraphUiInteraction,

    pub arena: bumpalo::Bump,
}

impl MainUi {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            graph_ui: GraphUi::default(),
            ui_context: UiContext::new(ctx),
            graph_ui_interaction: GraphUiInteraction::default(),
            arena: bumpalo::Bump::new(),
        }
    }

    pub fn ui_context(&self) -> UiContext {
        self.ui_context.clone()
    }

    fn empty(&mut self, app_data: &mut AppData) {
        self.graph_ui.reset();
        app_data.empty_graph();
    }

    fn save(&mut self, app_data: &mut AppData) {
        app_data.save_graph();
    }

    pub fn load(&mut self, app_data: &mut AppData) {
        self.graph_ui.reset();
        app_data.load_graph();
    }

    pub fn test_graph(&mut self, app_data: &mut AppData) {
        self.graph_ui.reset();
        app_data.load_test_graph();
    }

    pub fn run_graph(&mut self, app_data: &mut AppData) {
        app_data.run_graph();
    }

    pub fn render(&mut self, app_data: &mut AppData, ctx: &egui::Context) {
        app_data.update_status();

        egui::TopBottomPanel::top("top_panel")
            // .frame(Frame {
            //     ..Default::default()
            // })
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

        egui::TopBottomPanel::bottom("run_panel")
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Run").clicked() {
                        self.run_graph(app_data);
                    }
                });
            });

        let style = Style::new(1.0);

        egui::CentralPanel::default()
            // .frame(Frame {
            //     inner_margin: 0.0.into(),
            //     stroke: style.inactive_bg_stroke,
            //     corner_radius: style.corner_radius.into(),
            //     outer_margin: style.padding.into(),
            //     ..Default::default()
            // })
            .frame(Frame::NONE)
            .show(ctx, |ui| {
                // let rect = ui.available_rect_before_wrap();
                // let mut graph_ui = ui.new_child(
                //     egui::UiBuilder::new()
                //         .id_salt("graph_ui")
                //         .max_rect(rect)
                //         .sense(Sense::hover()),
                // );
                // graph_ui.set_clip_rect(rect);

                // ui.clip_rect();

                let mut gui = Gui::new(ui, style);

                self.graph_ui.render(
                    &mut gui,
                    &mut app_data.view_graph,
                    app_data.execution_stats.as_ref(),
                    &app_data.func_lib,
                    &mut self.graph_ui_interaction,
                );
            });

        app_data.handle_graph_ui_actions(&self.graph_ui_interaction);

        self.graph_ui_interaction.clear();
        self.arena.reset();
    }
}
