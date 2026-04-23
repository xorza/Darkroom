mod common;
mod config;
mod gui;
mod init;
mod input;
mod model;
mod script;
mod session;
mod tui;
mod ui_host;

use anyhow::Result;
use eframe::{NativeOptions, egui};
use std::sync::Arc;

use std::sync::atomic::AtomicBool;

use crate::gui::Gui;
use crate::gui::main_window::MainWindow;
use crate::session::Session;
use crate::tui::{MainTui, TuiUiHost};
use crate::ui_host::EguiUiHost;

#[tokio::main]
async fn main() -> Result<()> {
    init::init()?;

    if std::env::args().skip(1).any(|a| a == "tui") {
        let (mut app, should_close) = PrismApp::new_tui();
        return app.run_tui(&should_close);
    }

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
            Ok(Box::new(PrismApp::new_gui(&cc.egui_ctx)))
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

/// Runtime UI mode. `Headless` is scaffolding for the future
/// non-interactive entry point; `Gui` runs under eframe and `Tui`
/// drives a console loop from `main`.
#[derive(Debug)]
#[allow(dead_code)]
enum Frontend {
    Headless,
    Tui(Box<MainTui>),
    Gui(Box<MainWindow>),
}

impl Frontend {
    fn as_gui_mut(&mut self) -> &mut MainWindow {
        match self {
            Frontend::Gui(main_window) => main_window,
            _ => unreachable!("eframe::App methods should only run when the frontend is Gui"),
        }
    }

    fn as_tui_mut(&mut self) -> &mut MainTui {
        match self {
            Frontend::Tui(main_tui) => main_tui,
            _ => unreachable!("run_tui called when the frontend is not Tui"),
        }
    }
}

#[derive(Debug)]
struct PrismApp {
    session: Session,
    frontend: Frontend,
}

impl PrismApp {
    fn new_gui(ctx: &egui::Context) -> Self {
        Self {
            session: Session::new(EguiUiHost::new(ctx)),
            frontend: Frontend::Gui(Box::new(MainWindow::new(EguiUiHost::new(ctx)))),
        }
    }

    fn new_tui() -> (Self, Arc<AtomicBool>) {
        let host = TuiUiHost::new();
        let should_close = host.should_close();
        let app = Self {
            session: Session::new(host),
            frontend: Frontend::Tui(Box::new(MainTui::new())),
        };
        (app, should_close)
    }

    fn run_tui(&mut self, should_close: &AtomicBool) -> Result<()> {
        let result = self
            .frontend
            .as_tui_mut()
            .run(&mut self.session, should_close);
        self.session.exit();
        result
    }
}

impl eframe::App for PrismApp {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frontend.as_gui_mut().pre_frame(&mut self.session);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let main_window = self.frontend.as_gui_mut();
        let mut gui = Gui::new(ui, &main_window.style);
        main_window.render(&mut self.session, &mut gui);
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
