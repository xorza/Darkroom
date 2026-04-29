use egui::{Color32, FontId, Response};

use crate::gui::Gui;

#[derive(Debug)]
pub struct Label {
    text: String,
    color: Option<Color32>,
    font: Option<FontId>,
    truncate: bool,
    selectable: Option<bool>,
}

impl Label {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            color: None,
            font: None,
            truncate: false,
            selectable: None,
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

    /// Override `style.interaction.selectable_labels`. Defaults to
    /// the style value (egui's default is `true`). When selectable,
    /// the label registers `Sense::click_and_drag` over its galley
    /// rect — pass `false` for chrome labels (titles, headers,
    /// captions) so they don't intercept pointer events meant for an
    /// enclosing draggable container.
    pub fn selectable(mut self, selectable: bool) -> Self {
        self.selectable = Some(selectable);
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
        if let Some(selectable) = self.selectable {
            label = label.selectable(selectable);
        }
        gui.ui_raw().add(label)
    }
}
