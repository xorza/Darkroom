use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::widgets::StatusPanel;

#[derive(Debug, Default)]
pub struct LogUi;

impl LogUi {
    pub fn render(&self, gui: &mut Gui, status: &str) {
        StatusPanel::new(StableId::new("log_ui_status_panel"), status).show(gui);
    }
}
