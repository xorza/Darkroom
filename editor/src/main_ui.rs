use std::sync::Arc;

use eframe::egui;
use graph::graph::Binding;
use graph::prelude::{TestFuncHooks, test_func_lib, test_graph};
use graph::worker::WorkerMessage;

use crate::gui::graph::{GraphUi, GraphUiAction, GraphUiInteraction};
use crate::model::{AppData, ViewGraph};

#[derive(Clone, Debug)]
pub struct UiContext {
    ctx: egui::Context,
}

impl UiContext {
    pub fn new(ctx: &egui::Context) -> Self {
        Self { ctx: ctx.clone() }
    }

    pub fn request_repaint(&self) {
        self.ctx.request_repaint();
    }
}

#[derive(Debug)]
pub struct MainUi {
    pub graph_ui: GraphUi,
    pub ui_context: UiContext,
    pub graph_ui_interaction: GraphUiInteraction,
}

impl MainUi {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            graph_ui: GraphUi::default(),
            ui_context: UiContext::new(ctx),
            graph_ui_interaction: GraphUiInteraction::default(),
        }
    }

    pub fn ui_context(&self) -> UiContext {
        self.ui_context.clone()
    }

    fn set_graph_view(
        &mut self,
        app_data: &mut AppData,
        view_graph: ViewGraph,
        status: impl Into<String>,
    ) {
        view_graph
            .validate()
            .expect("graph should be valid before storing in app state");
        app_data.view_graph = view_graph;
        self.graph_ui.reset();
        app_data.set_status(status);
        app_data.worker.send(WorkerMessage::Clear);
        app_data.graph_updated = true;
    }

    fn empty(&mut self, app_data: &mut AppData) {
        let view_graph = ViewGraph::default();
        self.set_graph_view(app_data, view_graph, "Created new graph");
    }

    fn save(&mut self, app_data: &mut AppData) {
        assert!(
            app_data.current_path.extension().is_some(),
            "graph save path must include a file extension"
        );
        match app_data
            .view_graph
            .serialize_to_file(&app_data.current_path)
        {
            Ok(()) => app_data.set_status(format!(
                "Saved graph to {}",
                app_data.current_path.display()
            )),
            Err(err) => app_data.set_status(format!("Save failed: {err}")),
        }
    }

    pub fn load(&mut self, app_data: &mut AppData) {
        assert!(
            app_data.current_path.extension().is_some(),
            "graph load path must include a file extension"
        );
        match ViewGraph::deserialize_from_file(&app_data.current_path) {
            Ok(graph_view) => self.set_graph_view(
                app_data,
                graph_view,
                format!("Loaded graph from {}", app_data.current_path.display()),
            ),
            Err(err) => app_data.set_status(format!("Load failed: {err}")),
        }
    }

    fn sample_test_hooks(app_data: &AppData) -> TestFuncHooks {
        let print_output = Arc::clone(&app_data.print_output);
        TestFuncHooks {
            get_a: Arc::new(|| 21),
            get_b: Arc::new(|| 2),
            print: Arc::new(move |value| {
                print_output.store(Some(Arc::new(value.to_string())));
            }),
        }
    }

    pub fn test_graph(&mut self, app_data: &mut AppData) {
        let mut graph = test_graph();
        graph.by_name_mut("sum").unwrap().inputs[0].binding = Binding::Const(132.into());

        let func_lib = test_func_lib(Self::sample_test_hooks(app_data));
        let graph_view = ViewGraph::from_graph(&graph);
        app_data.func_lib = func_lib;
        self.set_graph_view(app_data, graph_view, "Loaded sample test graph");
    }

    pub fn run_graph(&mut self, app_data: &mut AppData) {
        if app_data.view_graph.graph.nodes.is_empty() {
            app_data.set_status("Run failed: no compute graph loaded");
            return;
        }

        if app_data.graph_updated {
            app_data
                .worker
                .update(app_data.view_graph.graph.clone(), app_data.func_lib.clone());
        }
        app_data.worker.event();
    }

    pub fn render(&mut self, app_data: &mut AppData, ctx: &egui::Context) {
        app_data.poll_compute_status();

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
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

        egui::CentralPanel::default().show(ctx, |ui| {
            self.graph_ui.render(
                ui,
                &mut app_data.view_graph,
                &app_data.func_lib,
                &mut self.graph_ui_interaction,
            );
        });

        egui::TopBottomPanel::bottom("status_panel").show(ctx, |ui| {
            ui.label(&app_data.status);
        });
        egui::TopBottomPanel::bottom("run_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Run").clicked() {
                    self.run_graph(app_data);
                }
            });
        });

        if !self.graph_ui_interaction.actions.is_empty() {
            let node_ids_to_invalidate = self.graph_ui_interaction.actions.iter().filter_map(
                |(node_id, graph_ui_action)| match graph_ui_action {
                    GraphUiAction::CacheToggled => None,
                    GraphUiAction::InputChanged | GraphUiAction::NodeRemoved => {
                        app_data.graph_updated = true;
                        Some(*node_id)
                    }
                },
            );
            app_data.worker.invalidate_caches(node_ids_to_invalidate);
        }
    }
}
