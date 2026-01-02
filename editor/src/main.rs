#![allow(dead_code)]
#![allow(unused_imports)]

mod gui;
mod init;
mod model;

use anyhow::Result;
use arc_swap::ArcSwapOption;
use common::Shared;
use eframe::{NativeOptions, egui};
use graph::execution_graph::ExecutionGraph;
use graph::graph::NodeId;
use graph::prelude::{FuncLib, TestFuncHooks, test_func_lib, test_graph};
use graph::worker::{Worker, WorkerMessage};
use pollster::{FutureExt, block_on};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Arc;

use crate::gui::graph::GraphUiAction;

#[tokio::main]
async fn main() -> Result<()> {
    init::init()?;

    let app_icon = load_window_icon();
    let options = NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_icon(app_icon)
            .with_app_id("scenarium-egui"),
        ..Default::default()
    };

    eframe::run_native(
        "Scenarium",
        options,
        Box::new(|cc| {
            configure_fonts(&cc.egui_ctx);
            configure_visuals(&cc.egui_ctx);
            Ok(Box::new(ScenariumApp::new(&cc.egui_ctx)))
        }),
    )?;

    Ok(())
}

fn load_window_icon() -> Arc<egui::IconData> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
        .expect("window icon PNG should be a valid RGBA image");
    Arc::new(icon)
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let font_data = egui::FontData::from_static(include_bytes!("../assets/Raleway-Medium.ttf"));
    fonts
        .font_data
        .insert("Raleway".to_owned(), Arc::new(font_data));

    let proportional = fonts
        .families
        .get_mut(&egui::FontFamily::Proportional)
        .expect("proportional font family should exist in default font definitions");
    proportional.insert(0, "Raleway".to_owned());

    ctx.set_fonts(fonts);
}

fn configure_visuals(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals.override_text_color = Some(egui::Color32::from_rgb(200, 200, 200));
    ctx.set_style(style);
}

#[derive(Debug)]
struct ScenariumApp {
    worker: Worker,
    func_lib: FuncLib,
    view_graph: model::ViewGraph,
    graph_path: PathBuf,
    graph_ui: gui::graph::GraphUi,
    graph_updated: bool,

    print_output: Arc<ArcSwapOption<String>>,
    updated_status: Arc<ArcSwapOption<String>>,
    ui_context: egui::Context,
    status: String,
}

impl ScenariumApp {
    fn new(ui_context: &egui::Context) -> Self {
        let graph_path = Self::default_path();
        let updated_status: Arc<ArcSwapOption<String>> = Arc::new(ArcSwapOption::empty());
        let print_output: Arc<ArcSwapOption<String>> = Arc::new(ArcSwapOption::empty());
        let worker = Self::create_worker(&updated_status, &print_output, ui_context.clone());

        let mut result = Self {
            worker,
            func_lib: FuncLib::default(),
            view_graph: model::ViewGraph::default(),
            graph_path,
            graph_ui: gui::graph::GraphUi::default(),
            graph_updated: false,

            print_output,
            updated_status,
            ui_context: ui_context.clone(),
            status: "".to_string(),
        };

        result.test_graph();
        result.load();

        result
    }

    fn create_worker(
        updated_status: &Arc<ArcSwapOption<String>>,
        print_output: &Arc<ArcSwapOption<String>>,
        ui_context: egui::Context,
    ) -> Worker {
        let updated_status = Arc::clone(updated_status);
        let print_output = Arc::clone(print_output);

        Worker::new(move |result| {
            match result {
                Ok(stats) => {
                    let print_output = print_output.swap(None);
                    let summary = format!(
                        "({} nodes, {:.0}s)",
                        stats.executed_nodes, stats.elapsed_secs
                    );
                    let message = if let Some(print_output) = print_output {
                        format!("Compute output: {print_output} {summary}")
                    } else {
                        format!("Compute finished {summary}")
                    };
                    updated_status.store(Some(Arc::new(message)));
                }
                Err(err) => {
                    updated_status.store(Some(Arc::new(format!("Compute failed: {err}"))));
                }
            }

            ui_context.request_repaint();
        })
    }

    fn default_path() -> PathBuf {
        std::env::temp_dir().join("scenarium-graph.lua")
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.status = message.into();
    }

    fn poll_compute_status(&mut self) {
        let updated_status = self.updated_status.swap(None);
        if let Some(updated_status) = updated_status {
            self.set_status(updated_status.as_ref());
        }
    }

    fn set_graph_view(&mut self, view_graph: model::ViewGraph, status: impl Into<String>) {
        view_graph
            .validate()
            .expect("graph should be valid before storing in app state");
        self.view_graph = view_graph;
        self.graph_ui.reset();
        self.set_status(status);
        self.worker.send(WorkerMessage::Clear);
        self.graph_updated = true;
    }

    fn empty(&mut self) {
        let view_graph = model::ViewGraph::default();
        self.set_graph_view(view_graph, "Created new graph");
    }

    fn save(&mut self) {
        assert!(
            self.graph_path.extension().is_some(),
            "graph save path must include a file extension"
        );
        match self.view_graph.serialize_to_file(&self.graph_path) {
            Ok(()) => self.set_status(format!("Saved graph to {}", self.graph_path.display())),
            Err(err) => self.set_status(format!("Save failed: {err}")),
        }
    }

    fn load(&mut self) {
        assert!(
            self.graph_path.extension().is_some(),
            "graph load path must include a file extension"
        );
        match model::ViewGraph::deserialize_from_file(&self.graph_path) {
            Ok(graph_view) => self.set_graph_view(
                graph_view,
                format!("Loaded graph from {}", self.graph_path.display()),
            ),
            Err(err) => self.set_status(format!("Load failed: {err}")),
        }
    }

    fn test_graph(&mut self) {
        let graph = test_graph();
        let func_lib = test_func_lib(self.sample_test_hooks());
        let graph_view = model::ViewGraph::from_graph(&graph);
        self.func_lib = func_lib;
        self.set_graph_view(graph_view, "Loaded sample test graph");
    }

    fn sample_test_hooks(&self) -> TestFuncHooks {
        let print_output = Arc::clone(&self.print_output);
        TestFuncHooks {
            get_a: Arc::new(|| 21),
            get_b: Arc::new(|| 2),
            print: Arc::new(move |value| {
                print_output.store(Some(Arc::new(value.to_string())));
            }),
        }
    }

    fn run_graph(&mut self) {
        if self.view_graph.graph.nodes.is_empty() {
            self.set_status("Run failed: no compute graph loaded");
            return;
        }

        if self.graph_updated {
            self.worker
                .update(self.view_graph.graph.clone(), self.func_lib.clone());
        }
        self.worker.event();
    }
}

impl eframe::App for ScenariumApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_compute_status();
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
                        self.empty();
                        ui.close();
                    }
                    if ui.button("Save").clicked() {
                        self.save();
                        ui.close();
                    }
                    if ui.button("Load").clicked() {
                        self.load();
                        ui.close();
                    }
                    if ui.button("Test").clicked() {
                        self.test_graph();
                        ui.close();
                    }
                });
            });
        });

        let mut graph_interaction = gui::graph::GraphUiInteraction::default();
        egui::CentralPanel::default().show(ctx, |ui| {
            graph_interaction = self
                .graph_ui
                .render(ui, &mut self.view_graph, &self.func_lib);
        });

        egui::TopBottomPanel::bottom("status_panel").show(ctx, |ui| {
            ui.label(&self.status);
        });
        egui::TopBottomPanel::bottom("run_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Run").clicked() {
                    self.run_graph();
                }
            });
        });

        if !graph_interaction.actions.is_empty() {
            let node_ids_to_invalidate =
                graph_interaction
                    .actions
                    .iter()
                    .filter_map(|(node_id, graph_ui_action)| match graph_ui_action {
                        GraphUiAction::CacheToggled => None,
                        GraphUiAction::InputChanged | GraphUiAction::NodeRemoved => {
                            self.graph_updated = true;
                            Some(*node_id)
                        }
                    });
            self.worker.invalidate_caches(node_ids_to_invalidate);
        }
    }
}
