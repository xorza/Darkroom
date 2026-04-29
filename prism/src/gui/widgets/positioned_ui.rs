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
        let initial_rect = if let Some(rect) = self.rect {
            rect
        } else {
            // First pass: measure content size using invisible UI
            let content_size = gui
                .ui_raw()
                .ctx()
                .memory(|mem| mem.data.get_temp::<Vec2>(self.id.id()));

            let top_left = if let Some(size) = content_size {
                compute_pivot_offset(self.position, size, self.pivot)
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
                let args = gui.child_args();
                let result = args.enter(gui.ui_raw(), add_contents);

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
}

/// Top-left of a `size`-bounded rect anchored at `position` with the given
/// `pivot`. Pure helper — extracted from `PositionedUi::show` so the pivot
/// math can be unit-tested without an `egui::Ui`.
pub(crate) fn compute_pivot_offset(position: Pos2, size: Vec2, pivot: Align2) -> Pos2 {
    let x = match pivot.x() {
        egui::Align::Min => position.x,
        egui::Align::Center => position.x - size.x / 2.0,
        egui::Align::Max => position.x - size.x,
    };
    let y = match pivot.y() {
        egui::Align::Min => position.y,
        egui::Align::Center => position.y - size.y / 2.0,
        egui::Align::Max => position.y - size.y,
    };
    Pos2::new(x, y)
}

#[cfg(test)]
mod tests {
    use super::compute_pivot_offset;
    use egui::{Align2, Pos2, Vec2};

    #[test]
    fn left_top_pivot_is_identity() {
        let p = compute_pivot_offset(
            Pos2::new(10.0, 20.0),
            Vec2::new(40.0, 30.0),
            Align2::LEFT_TOP,
        );
        assert_eq!(p, Pos2::new(10.0, 20.0));
    }

    #[test]
    fn right_bottom_pivot_subtracts_full_size() {
        let p = compute_pivot_offset(
            Pos2::new(100.0, 200.0),
            Vec2::new(40.0, 30.0),
            Align2::RIGHT_BOTTOM,
        );
        assert_eq!(p, Pos2::new(60.0, 170.0));
    }

    #[test]
    fn center_center_pivot_subtracts_half_size() {
        let p = compute_pivot_offset(
            Pos2::new(100.0, 200.0),
            Vec2::new(40.0, 30.0),
            Align2::CENTER_CENTER,
        );
        assert_eq!(p, Pos2::new(80.0, 185.0));
    }

    #[test]
    fn mixed_pivot_left_bottom() {
        let p = compute_pivot_offset(
            Pos2::new(50.0, 50.0),
            Vec2::new(20.0, 10.0),
            Align2::LEFT_BOTTOM,
        );
        assert_eq!(p, Pos2::new(50.0, 40.0));
    }
}
