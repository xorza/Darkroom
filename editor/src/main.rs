#![allow(dead_code)]
#![allow(unused_imports)]

mod gui;
mod init;
mod model;

use anyhow::Result;
use eframe::{NativeOptions, egui};
use graph::execution_graph::ExecutionGraph;
use graph::prelude::{FuncLib, Graph, TestFuncHooks, test_func_lib, test_graph};
use pollster::block_on;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
            Ok(Box::new(ScenariumApp::default()))
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
    graph: Graph,
    func_lib: FuncLib,
    execution_graph: ExecutionGraph,
    graph_view: model::GraphView,
    graph_path: PathBuf,
    last_status: Option<String>,
    compute_status: Arc<Mutex<Option<String>>>,
    graph_ui: gui::graph::GraphUi,
}

impl Default for ScenariumApp {
    fn default() -> Self {
        let graph_path = Self::default_graph_path();

        let mut result = Self {
            graph: Graph::default(),
            func_lib: FuncLib::default(),
            execution_graph: ExecutionGraph::default(),
            graph_view: model::GraphView::default(),
            graph_path,
            last_status: None,
            compute_status: Arc::new(Mutex::new(None)),
            graph_ui: gui::graph::GraphUi::default(),
        };

        result.test_graph();
        result.load();

        result
    }
}

impl ScenariumApp {
    fn default_graph_path() -> PathBuf {
        std::env::temp_dir().join("scenarium-graph.lua")
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.last_status = Some(message.into());
    }

    fn set_graph_view(&mut self, graph_view: model::GraphView, status: impl Into<String>) {
        graph_view
            .validate()
            .expect("graph should be valid before storing in app state");
        self.graph_view = graph_view;
        self.execution_graph = ExecutionGraph::default();
        self.graph_ui.reset();
        self.set_status(status);
    }

    fn empty(&mut self) {
        let graph = model::GraphView::default();
        self.set_graph_view(graph, "Created new graph");
    }

    fn save(&mut self) {
        assert!(
            self.graph_path.extension().is_some(),
            "graph save path must include a file extension"
        );
        match self.graph_view.serialize_to_file(&self.graph_path) {
            Ok(()) => self.set_status(format!("Saved graph to {}", self.graph_path.display())),
            Err(err) => self.set_status(format!("Save failed: {err}")),
        }
    }

    fn load(&mut self) {
        assert!(
            self.graph_path.extension().is_some(),
            "graph load path must include a file extension"
        );
        match model::GraphView::deserialize_from_file(&self.graph_path) {
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
        let graph_view = model::GraphView::from_graph(&graph, &func_lib);
        self.graph = graph;
        self.func_lib = func_lib;
        self.set_graph_view(graph_view, "Loaded sample test graph");
    }

    fn sample_test_hooks(&self) -> TestFuncHooks {
        let status = Arc::clone(&self.compute_status);
        TestFuncHooks {
            get_a: Box::new(|| 21),
            get_b: Box::new(|| 2),
            print: Box::new(move |value| {
                let mut slot = status.lock().expect("Compute status mutex poisoned");
                *slot = Some(format!("Compute output: {}", value));
            }),
        }
    }

    fn run_graph(&mut self) {
        if self.graph_view.nodes.is_empty() {
            self.set_status("Run failed: no compute graph loaded");
            return;
        }

        self.graph = self.graph_view.to_graph(&self.func_lib);
        {
            let mut slot = self
                .compute_status
                .lock()
                .expect("Compute status mutex poisoned");
            *slot = None;
        }

        let result = self
            .execution_graph
            .update(&self.graph, &self.func_lib)
            .and_then(|()| self.execution_graph.execute(&self.graph, &self.func_lib));

        match result {
            Ok(()) => {
                let status = self
                    .compute_status
                    .lock()
                    .expect("Compute status mutex poisoned")
                    .clone();
                if let Some(status) = status {
                    self.set_status(status);
                } else {
                    self.set_status("Compute finished");
                }
            }
            Err(err) => self.set_status(format!("Compute failed: {err}")),
        }
    }
}

impl eframe::App for ScenariumApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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

        egui::CentralPanel::default().show(ctx, |ui| {
            self.graph_ui.render(ui, &mut self.graph_view);
        });

        egui::TopBottomPanel::bottom("status_panel").show(ctx, |ui| {
            if let Some(status) = self.last_status.as_deref() {
                ui.label(status);
            }
        });
        egui::TopBottomPanel::bottom("run_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Run").clicked() {
                    self.run_graph();
                }
            });
        });
    }
}
