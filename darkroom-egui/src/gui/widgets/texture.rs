//! Texture uploads via `egui::Context`, wrapped so app-layer code never
//! touches `Context` directly.

use egui::{ColorImage, TextureHandle, TextureOptions};

use crate::gui::Gui;

#[derive(Debug)]
pub struct Texture {
    name: String,
    image: ColorImage,
    options: TextureOptions,
}

impl Texture {
    pub fn new(name: impl Into<String>, image: ColorImage) -> Self {
        Self {
            name: name.into(),
            image,
            options: TextureOptions::LINEAR,
        }
    }

    pub fn options(mut self, options: TextureOptions) -> Self {
        self.options = options;
        self
    }

    pub fn load(self, gui: &mut Gui<'_>) -> TextureHandle {
        gui.ui_raw()
            .ctx()
            .load_texture(self.name, self.image, self.options)
    }
}
