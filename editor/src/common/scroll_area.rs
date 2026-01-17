use egui::Id;

use crate::gui::Gui;

#[derive(Debug)]
pub struct ScrollArea {
    id: Option<Id>,
    max_height: Option<f32>,
    max_width: Option<f32>,
    horizontal: bool,
    vertical: bool,
}

impl Default for ScrollArea {
    fn default() -> Self {
        Self {
            id: None,
            max_height: None,
            max_width: None,
            horizontal: false,
            vertical: true,
        }
    }
}

impl ScrollArea {
    pub fn vertical() -> Self {
        Self::default()
    }

    pub fn horizontal() -> Self {
        Self {
            horizontal: true,
            vertical: false,
            ..Default::default()
        }
    }

    pub fn both() -> Self {
        Self {
            horizontal: true,
            vertical: true,
            ..Default::default()
        }
    }

    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    pub fn max_height(mut self, max_height: f32) -> Self {
        self.max_height = Some(max_height);
        self
    }

    pub fn max_width(mut self, max_width: f32) -> Self {
        self.max_width = Some(max_width);
        self
    }

    pub fn show<R>(self, gui: &mut Gui<'_>, add_contents: impl FnOnce(&mut Gui<'_>) -> R) -> R {
        let style = gui.style.clone();
        let scale = gui.scale();

        let mut scroll_area = egui::ScrollArea::new([self.horizontal, self.vertical]);

        if let Some(id) = self.id {
            scroll_area = scroll_area.id_salt(id);
        }

        if let Some(max_height) = self.max_height {
            scroll_area = scroll_area.max_height(max_height);
        }

        if let Some(max_width) = self.max_width {
            scroll_area = scroll_area.max_width(max_width);
        }

        scroll_area = scroll_area.auto_shrink([false, false]);

        scroll_area
            .show(gui.ui(), |ui| {
                let mut child_gui = Gui::new(ui, &style);
                child_gui.set_scale(scale);
                add_contents(&mut child_gui)
            })
            .inner
    }
}
