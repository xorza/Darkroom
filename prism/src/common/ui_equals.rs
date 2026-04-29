use egui::{Pos2, Vec2};

pub trait UiEquals {
    fn ui_equals(&self, other: Self) -> bool;
}

impl UiEquals for f32 {
    /// Relative tolerance: 1%. Loose enough that trackpad-pinch
    /// micro-deltas don't churn the viewport-emit / galley-rebuild
    /// gates, well below the user-visible JND for text size (~5%).
    fn ui_equals(&self, other: Self) -> bool {
        let diff = (self - other).abs();
        let scale = self.abs().max(other.abs()).max(1.0);

        diff / scale <= 0.005
    }
}

impl UiEquals for Vec2 {
    fn ui_equals(&self, other: Self) -> bool {
        (*self - other).length_sq() <= 1.0
    }
}

impl UiEquals for Pos2 {
    fn ui_equals(&self, other: Self) -> bool {
        (*self - other).length_sq() <= 1.0
    }
}
