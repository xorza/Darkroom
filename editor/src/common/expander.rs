use egui::{CollapsingResponse, Id};

use crate::gui::Gui;

#[derive(Debug)]
pub struct Expander {
    id: Id,
    text: String,
    default_open: bool,
}

impl Expander {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            id: Id::new(&text),
            text,
            default_open: false,
        }
    }

    pub fn id(mut self, id: Id) -> Self {
        self.id = id;
        self
    }

    pub fn default_open(mut self, default_open: bool) -> Self {
        self.default_open = default_open;
        self
    }

    pub fn show<R>(
        self,
        gui: &mut Gui<'_>,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> CollapsingResponse<R> {
        let style = gui.style.clone();
        let icon_spacing = gui.style.small_padding;
        gui.ui().spacing_mut().icon_spacing = icon_spacing;
        egui::CollapsingHeader::new(&self.text)
            .id_salt(self.id)
            .default_open(self.default_open)
            .show(gui.ui(), |ui| {
                let mut gui = Gui::new(ui, &style);
                add_contents(&mut gui)
            })
    }
}
