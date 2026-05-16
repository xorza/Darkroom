use egui::{Response, TextureId, Vec2};

use crate::gui::Gui;

#[derive(Debug)]
pub struct Image {
    texture_id: TextureId,
    size: Vec2,
}

impl Image {
    pub fn new(texture_id: TextureId, size: Vec2) -> Self {
        Self { texture_id, size }
    }

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        gui.ui_raw().image((self.texture_id, self.size))
    }
}
