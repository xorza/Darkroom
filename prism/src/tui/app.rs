use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;

use crate::app_config::AppConfig;
use crate::session::Session;
use crate::tui::main_tui::MainTui;
use crate::tui::ui_host::TuiUiHost;

#[derive(Debug)]
pub struct TuiApp {
    session: Session,
    main_tui: MainTui,
    shutdown: Arc<AtomicBool>,
}

impl TuiApp {
    pub fn new(app_config: AppConfig) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        Self {
            session: Session::new(TuiUiHost::new(shutdown.clone()), app_config),
            main_tui: MainTui::new(),
            shutdown,
        }
    }

    pub fn run(&mut self) -> Result<()> {
        let result = self.main_tui.run(&mut self.session, &self.shutdown);
        self.session.exit();
        result
    }
}
