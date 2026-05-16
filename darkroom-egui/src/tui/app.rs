use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use tokio::sync::Notify;

use crate::launch_config::LaunchConfig;
use crate::session::Session;
use crate::tui::main_tui::MainTui;
use crate::tui::ui_host::TuiUiHost;

#[derive(Debug)]
pub struct TuiApp {
    session: Session,
    main_tui: MainTui,
    wake: Arc<Notify>,
    shutdown: Arc<AtomicBool>,
}

impl TuiApp {
    pub fn new(launch_config: LaunchConfig) -> Self {
        let wake = Arc::new(Notify::new());
        let shutdown = Arc::new(AtomicBool::new(false));
        let host = TuiUiHost::new(wake.clone(), shutdown.clone());
        Self {
            session: Session::new(host, launch_config),
            main_tui: MainTui::new(),
            wake,
            shutdown,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let result = self
            .main_tui
            .run(&mut self.session, &self.wake, &self.shutdown)
            .await;
        self.session.exit();
        result
    }
}
