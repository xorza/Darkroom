use crate::common::StableId;
use crate::gui::Gui;

#[derive(Debug)]
#[must_use = "ScrollArea does nothing until .show() is called"]
pub struct ScrollArea {
    id: StableId,
    min_height: Option<f32>,
    max_height: Option<f32>,
    max_width: Option<f32>,
    horizontal: bool,
    vertical: bool,
}

impl ScrollArea {
    pub fn vertical(id: StableId) -> Self {
        Self {
            id,
            min_height: None,
            max_height: None,
            max_width: None,
            horizontal: false,
            vertical: true,
        }
    }

    pub fn horizontal(id: StableId) -> Self {
        Self {
            id,
            min_height: None,
            max_height: None,
            max_width: None,
            horizontal: true,
            vertical: false,
        }
    }

    pub fn both(id: StableId) -> Self {
        Self {
            id,
            min_height: None,
            max_height: None,
            max_width: None,
            horizontal: true,
            vertical: true,
        }
    }

    pub fn min_height(mut self, min_height: f32) -> Self {
        self.min_height = Some(min_height);
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

        let mut scroll_area =
            egui::ScrollArea::new([self.horizontal, self.vertical]).id_salt(self.id);

        if let Some(min_height) = self.min_height {
            scroll_area = scroll_area.min_scrolled_height(min_height);
        }

        if let Some(max_height) = self.max_height {
            scroll_area = scroll_area.max_height(max_height);
        }

        if let Some(max_width) = self.max_width {
            scroll_area = scroll_area.max_width(max_width);
        }

        scroll_area = scroll_area.auto_shrink([false, false]);

        scroll_area
            .show(gui.ui_raw(), |ui| {
                let mut child_gui = Gui::new_with_scale(ui, &style, scale);
                add_contents(&mut child_gui)
            })
            .inner
    }
}
