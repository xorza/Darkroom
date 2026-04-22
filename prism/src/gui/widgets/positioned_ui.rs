use egui::{Align2, InnerResponse, Pos2, Rect, Sense, Vec2};

use crate::common::StableId;
use crate::gui::Gui;

/// A lightweight alternative to `egui::Area` that places UI at a specified position
/// without using egui's Area (which has memory/state overhead).
#[derive(Debug)]
pub struct PositionedUi {
    id: StableId,
    position: Pos2,
    pivot: Align2,
    max_size: Vec2,
    interactable: bool,
    rect: Option<Rect>,
}

impl PositionedUi {
    pub fn new(id: StableId, position: Pos2) -> Self {
        Self {
            id,
            position,
            pivot: Align2::LEFT_TOP,
            max_size: Vec2::new(f32::INFINITY, f32::INFINITY),
            interactable: false,
            rect: None,
        }
    }

    pub fn rect(mut self, rect: Rect) -> Self {
        self.rect = Some(rect);
        self
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

        let initial_rect = if let Some(rect) = self.rect {
            rect
        } else {
            // First pass: measure content size using invisible UI
            let content_size = gui
                .ui_raw()
                .ctx()
                .memory(|mem| mem.data.get_temp::<Vec2>(self.id.id()));

            let top_left = if let Some(size) = content_size {
                self.compute_top_left(size)
            } else {
                self.position
            };

            Rect::from_min_size(top_left, self.max_size)
        };

        let sense = if self.interactable {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        };

        gui.scope(self.id)
            .max_rect(initial_rect)
            .sense(sense)
            .show(|gui| {
                let mut child_gui = Gui::child(gui.ui_raw(), style);
                let result = add_contents(&mut child_gui);

                // Store measured size for next frame — but ONLY on the
                // final pass. egui runs the UI callback multiple times
                // within one logical frame (for auto-sizing /
                // discard-pass re-layout). If we wrote memory on every
                // pass, the stored size would update mid-frame, causing
                // the next pass to read a different size → different
                // top_left (for non-LEFT pivots) → widget positions
                // shift → "Widget rect changed id between passes"
                // warnings. `will_discard()` is false only on the last
                // pass.
                let ui = gui.ui_raw();
                if !ui.ctx().will_discard() {
                    let measured_size = ui.min_size();
                    ui.ctx().memory_mut(|mem| {
                        mem.data.insert_temp(self.id.id(), measured_size);
                    });
                }

                InnerResponse::new(result, ui.response())
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
