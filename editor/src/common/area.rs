use std::rc::Rc;

use egui::{Align2, Id, InnerResponse, Pos2, Response};

use crate::gui::{Gui, style::Style};

#[derive(Debug)]
pub struct Area {
    inner: egui::Area,
}

impl Area {
    pub fn new(id: Id) -> Self {
        Self {
            inner: egui::Area::new(id),
        }
    }

    pub fn sizing_pass(mut self, sizing_pass: bool) -> Self {
        self.inner = self.inner.sizing_pass(sizing_pass);
        self
    }

    pub fn default_width(mut self, width: f32) -> Self {
        self.inner = self.inner.default_width(width);
        self
    }

    pub fn movable(mut self, movable: bool) -> Self {
        self.inner = self.inner.movable(movable);
        self
    }

    pub fn interactable(mut self, interactable: bool) -> Self {
        self.inner = self.inner.interactable(interactable);
        self
    }

    pub fn fixed_pos(mut self, pos: Pos2) -> Self {
        self.inner = self.inner.fixed_pos(pos);
        self
    }

    pub fn pivot(mut self, pivot: Align2) -> Self {
        self.inner = self.inner.pivot(pivot);
        self
    }

    pub fn show<R>(
        self,
        gui: &mut Gui,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<R> {
        let style = gui.style.clone();
        self.inner.show(gui.ui().ctx(), |ui| {
            let mut gui = Gui::new(ui, &style);
            add_contents(&mut gui)
        })
    }
}
