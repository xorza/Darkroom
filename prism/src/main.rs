#![allow(dead_code)]
#![allow(unused_imports)]

mod app_data;
mod common;
mod elements;
mod gui;
mod init;
mod main_ui;
mod model;

use anyhow::Result;
use eframe::{NativeOptions, egui};
use egui::ViewportCommand;
use std::path::PathBuf;
use std::sync::Arc;

use crate::app_data::AppData;
use crate::main_ui::MainUi;

#[tokio::main]
async fn main() -> Result<()> {
    init::init()?;

    let app_icon = load_window_icon();
    let options = NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_icon(app_icon)
            .with_app_id("prism"),
        persist_window: true,
        ..Default::default()
    };

    eframe::run_native(
        "Prism",
        options,
        Box::new(|cc| {
            configure_fonts(&cc.egui_ctx);
            configure_visuals(&cc.egui_ctx);
            Ok(Box::new(ScenariumEditor::new(&cc.egui_ctx)))
        }),
    )?;

    Ok(())
}

fn load_window_icon() -> Arc<egui::IconData> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/prism.png"))
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
struct ScenariumEditor {
    app_data: AppData,
    main_ui: MainUi,
}

impl ScenariumEditor {
    fn new(ui_context: &egui::Context) -> Self {
        let main_ui = MainUi::new(ui_context);
        let mut app = Self {
            app_data: AppData::new(main_ui.ui_context()),
            main_ui,
        };

        app.init();

        app
    }

    fn init(&mut self) {}
}

impl eframe::App for ScenariumEditor {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.main_ui.render(&mut self.app_data, ctx);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        let color = visuals.panel_fill;
        [
            color.r() as f32 / 255.0,
            color.g() as f32 / 255.0,
            color.b() as f32 / 255.0,
            color.a() as f32 / 255.0,
        ]
    }

    fn on_exit(&mut self) {
        self.app_data.exit();
    }
}
