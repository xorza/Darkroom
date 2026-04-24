use anyhow::Result;

use crate::app_config::AppConfig;
use crate::session::Session;
use crate::tui::main_tui::MainTui;
use crate::tui::ui_host::TuiUiHost;

#[derive(Debug)]
pub struct TuiApp {
    session: Session,
    main_tui: MainTui,
}

impl TuiApp {
    pub fn new(app_config: AppConfig) -> Self {
        Self {
            session: Session::new(TuiUiHost::new(), app_config.script),
            main_tui: MainTui::new(),
        }
    }

    pub fn run(&mut self) -> Result<()> {
        let result = self.main_tui.run(&mut self.session);
        self.session.exit();
        result
    }
}
