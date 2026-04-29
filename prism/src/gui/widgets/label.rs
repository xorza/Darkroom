use egui::{Color32, FontId, Response};

use crate::gui::Gui;

#[derive(Debug)]
pub struct Label {
    text: String,
    color: Option<Color32>,
    font: Option<FontId>,
    truncate: bool,
}

impl Label {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            color: None,
            font: None,
            truncate: false,
        }
    }

    pub fn color(mut self, color: Color32) -> Self {
        self.color = Some(color);
        self
    }

    pub fn font(mut self, font: FontId) -> Self {
        self.font = Some(font);
        self
    }

    pub fn truncate(mut self, truncate: bool) -> Self {
        self.truncate = truncate;
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        let mut rich = egui::RichText::new(self.text);
        if let Some(color) = self.color {
            rich = rich.color(color);
        }
        if let Some(font) = self.font {
            rich = rich.font(font);
        }
        let mut label = egui::Label::new(rich);
        if self.truncate {
            label = label.truncate();
        }
        gui.ui_raw().add(label)
    }
}
