use std::rc::Rc;

use eframe::egui;

use crate::gui::debug::GuiDebug;
use crate::gui::main_window::MainWindow;
use crate::gui::style::Style;
use crate::gui::ui_host::EguiUiHost;
use crate::launch_config::LaunchConfig;
use crate::session::Session;
use crate::session::output::FrameOutput;

/// eframe app split across `logic` and `ui`:
///
/// - `logic` drains script + worker inbounds and forwards the
///   previous frame's [`FrameOutput`] to the worker. Eframe calls it
///   before every paint AND whenever `request_repaint` fires while
///   the window is hidden — so script-driven side effects (autorun,
///   intents) reach the worker even if no painting happens.
/// - `ui` renders, filling a fresh `FrameOutput`. The buffer lives on
///   the app so the next `logic` tick can consume it.
///
/// Net effect: handling user input is delayed by one frame (~16 ms)
/// in exchange for not blocking script side effects on a paint.
#[derive(Debug)]
pub struct GuiApp {
    session: Session,
    main_window: MainWindow,
    debug: GuiDebug,
    style: Rc<Style>,
    /// Filled by `ui`, consumed by the next `logic`. `clear()` runs
    /// at the end of `logic` so a sequence of hidden-only `logic`
    /// ticks doesn't reprocess stale intents.
    output: FrameOutput,
}

impl GuiApp {
    pub fn new(ctx: &egui::Context, launch_config: LaunchConfig) -> Self {
        let style = Rc::new(Style::from_file("style.toml").unwrap_or_default());
        Self {
            session: Session::new(EguiUiHost::new(ctx), launch_config),
            main_window: MainWindow::new(),
            debug: GuiDebug::new(),
            style,
            output: FrameOutput::default(),
        }
    }
}

impl eframe::App for GuiApp {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.session.drain_inbound();
        self.session.handle_output(&mut self.output);
        self.output.clear();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.debug.frame(ui.ctx());
        let cmd = self
            .main_window
            .render(&mut self.session, &mut self.output, &self.style, ui);
        if let Some(cmd) = cmd {
            self.main_window.handle_app_command(&mut self.session, cmd);
        }
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
