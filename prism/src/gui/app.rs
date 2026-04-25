use eframe::egui;

use crate::app_config::AppConfig;
use crate::gui::main_window::MainWindow;
use crate::gui::ui_host::EguiUiHost;
use crate::session::Session;

#[derive(Debug)]
pub struct GuiApp {
    session: Session,
    main_window: MainWindow,
}

impl GuiApp {
    pub fn new(ctx: &egui::Context, app_config: AppConfig) -> Self {
        Self {
            session: Session::new(EguiUiHost::new(ctx), app_config.script),
            main_window: MainWindow::new(),
        }
    }
}

impl eframe::App for GuiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.main_window.render(&mut self.session, ui);
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
