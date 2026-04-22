use egui::Vec2b;

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
    auto_shrink: Vec2b,
    stick_to_bottom: bool,
}

impl ScrollArea {
    pub fn vertical(id: StableId) -> Self {
        Self::new(id, false, true)
    }

    pub fn horizontal(id: StableId) -> Self {
        Self::new(id, true, false)
    }

    pub fn both(id: StableId) -> Self {
        Self::new(id, true, true)
    }

    fn new(id: StableId, horizontal: bool, vertical: bool) -> Self {
        Self {
            id,
            min_height: None,
            max_height: None,
            max_width: None,
            horizontal,
            vertical,
            auto_shrink: Vec2b::FALSE,
            stick_to_bottom: false,
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

    /// Shrink the scroll area to fit its content along the given axes.
    /// Default is `Vec2b::FALSE` (fill available space on both axes).
    pub fn auto_shrink(mut self, auto_shrink: Vec2b) -> Self {
        self.auto_shrink = auto_shrink;
        self
    }

    /// Scroll to the bottom on first render and keep it there when new
    /// content is appended.
    pub fn stick_to_bottom(mut self, stick: bool) -> Self {
        self.stick_to_bottom = stick;
        self
    }

    pub fn show<R>(self, gui: &mut Gui<'_>, add_contents: impl FnOnce(&mut Gui<'_>) -> R) -> R {
        let style = gui.style.clone();

        let mut scroll_area = egui::ScrollArea::new([self.horizontal, self.vertical])
            .id_salt(self.id)
            .auto_shrink(self.auto_shrink)
            .stick_to_bottom(self.stick_to_bottom);

        if let Some(min_height) = self.min_height {
            scroll_area = scroll_area.min_scrolled_height(min_height);
        }
        if let Some(max_height) = self.max_height {
            scroll_area = scroll_area.max_height(max_height);
        }
        if let Some(max_width) = self.max_width {
            scroll_area = scroll_area.max_width(max_width);
        }

        scroll_area
            .show(gui.ui_raw(), |ui| {
                let mut child_gui = Gui::child(ui, style);
                add_contents(&mut child_gui)
            })
            .inner
    }
}
