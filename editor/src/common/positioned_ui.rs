use egui::{Align2, Id, InnerResponse, Pos2, Rect, Sense, UiBuilder, Vec2};

use crate::gui::Gui;

/// A lightweight alternative to `egui::Area` that places UI at a specified position
/// without using egui's Area (which has memory/state overhead).
#[derive(Debug)]
pub struct PositionedUi {
    id: Id,
    position: Pos2,
    pivot: Align2,
    max_size: Vec2,
    interactable: bool,
}

impl PositionedUi {
    pub fn new(id: Id, position: Pos2) -> Self {
        Self {
            id,
            position,
            pivot: Align2::LEFT_TOP,
            max_size: Vec2::new(f32::INFINITY, f32::INFINITY),
            interactable: true,
        }
    }

    pub fn pivot(mut self, pivot: Align2) -> Self {
        self.pivot = pivot;
        self
    }

    pub fn max_size(mut self, max_size: Vec2) -> Self {
        self.max_size = max_size;
        self
    }

    pub fn interactable(mut self, interactable: bool) -> Self {
        self.interactable = interactable;
        self
    }

    pub fn show<R>(
        self,
        gui: &mut Gui,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<R> {
        let style = gui.style.clone();
        let scale = gui.scale();

        // First pass: measure content size using invisible UI
        let content_size = gui
            .ui()
            .ctx()
            .memory(|mem| mem.data.get_temp::<Vec2>(self.id));

        let top_left = if let Some(size) = content_size {
            self.compute_top_left(size)
        } else {
            self.position
        };

        let initial_rect = Rect::from_min_size(top_left, self.max_size);

        let builder = UiBuilder::new()
            .id_salt(self.id)
            .max_rect(initial_rect)
            .sense(if self.interactable {
                Sense::click_and_drag()
            } else {
                Sense::hover()
            });

        gui.ui().scope_builder(builder, |ui| {
            let mut child_gui = Gui::new(ui, &style);
            child_gui.set_scale(scale);
            let result = add_contents(&mut child_gui);

            // Store measured size for next frame
            let measured_size = ui.min_size();
            ui.ctx().memory_mut(|mem| {
                mem.data.insert_temp(self.id, measured_size);
            });

            result
        })
    }

    fn compute_top_left(&self, size: Vec2) -> Pos2 {
        let x = match self.pivot.x() {
            egui::Align::Min => self.position.x,
            egui::Align::Center => self.position.x - size.x / 2.0,
            egui::Align::Max => self.position.x - size.x,
        };
        let y = match self.pivot.y() {
            egui::Align::Min => self.position.y,
            egui::Align::Center => self.position.y - size.y / 2.0,
            egui::Align::Max => self.position.y - size.y,
        };
        Pos2::new(x, y)
    }
}
