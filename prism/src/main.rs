mod common;
mod config;
mod gui;
mod init;
mod input;
mod main_ui;
mod model;
mod script;
mod session;
mod ui_context;

use anyhow::Result;
use eframe::{NativeOptions, egui};
use std::sync::Arc;

use crate::gui::Gui;
use crate::main_ui::MainUi;
use crate::session::Session;
use crate::ui_context::UiContext;

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
            Ok(Box::new(PrismApp::new(&cc.egui_ctx)))
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

#[derive(Debug)]
struct PrismApp {
    session: Session,
    main_ui: MainUi,
}

impl PrismApp {
    fn new(ctx: &egui::Context) -> Self {
        Self {
            session: Session::new(UiContext::new(ctx)),
            main_ui: MainUi::new(UiContext::new(ctx)),
        }
    }
}

impl eframe::App for PrismApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut gui = Gui::new(ui, &self.main_ui.style);
        self.main_ui.render(&mut self.session, &mut gui);
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
        self.session.exit();
    }
}
