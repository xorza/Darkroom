use anyhow::Result;

use crate::session::Session;
use crate::tui::main_tui::MainTui;
use crate::tui::ui_host::TuiUiHost;

#[derive(Debug)]
pub struct TuiApp {
    session: Session,
    main_tui: MainTui,
}

impl TuiApp {
    pub fn new() -> Self {
        Self {
            session: Session::new(TuiUiHost::new()),
            main_tui: MainTui::new(),
        }
    }

    pub fn run(&mut self) -> Result<()> {
        let result = self.main_tui.run(&mut self.session);
        self.session.exit();
        result
    }
}
