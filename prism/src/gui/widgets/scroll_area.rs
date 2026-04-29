use egui::Vec2b;

use crate::common::StableId;
use crate::gui::Gui;

#[derive(Debug)]
#[must_use = "ScrollArea does nothing until .show() is called"]
pub struct ScrollArea {
    id: StableId,
    auto_shrink: Vec2b,
    stick_to_bottom: bool,
}

impl ScrollArea {
    pub fn vertical(id: StableId) -> Self {
        Self {
            id,
            auto_shrink: Vec2b::FALSE,
            stick_to_bottom: false,
        }
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
        let args = gui.child_args();

        egui::ScrollArea::vertical()
            .id_salt(self.id)
            .auto_shrink(self.auto_shrink)
            .stick_to_bottom(self.stick_to_bottom)
            .show(gui.ui_raw(), |ui| args.enter(ui, add_contents))
            .inner
    }
}
