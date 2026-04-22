use egui::Response;

use crate::gui::Gui;

#[derive(Debug)]
pub struct Separator;

impl Separator {
    pub fn new() -> Self {
        Self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        gui.ui_raw().separator()
    }
}

impl Default for Separator {
    fn default() -> Self {
        Self::new()
    }
}
