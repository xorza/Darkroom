use egui::{InnerResponse, Pos2};

use crate::common::StableId;
use crate::gui::Gui;

#[derive(Debug)]
pub struct Area {
    inner: egui::Area,
}

impl Area {
    pub fn new(id: StableId) -> Self {
        Self {
            inner: egui::Area::new(id.id()),
        }
    }

    pub fn fixed_pos(mut self, pos: Pos2) -> Self {
        self.inner = self.inner.fixed_pos(pos);
        self
    }

    pub fn order(mut self, order: egui::Order) -> Self {
        self.inner = self.inner.order(order);
        self
    }

    pub fn show<R>(
        self,
        gui: &mut Gui,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<R> {
        let args = gui.child_args();
        self.inner
            .show(gui.ui_raw().ctx(), |ui| args.enter(ui, add_contents))
    }
}
