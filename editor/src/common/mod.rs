use egui::{Pos2, Vec2};

pub mod bezier_helper;
pub mod button;
pub mod connection_bezier;
pub mod drag_value;
pub mod font;
pub mod toggle_button;

pub fn scale_changed(old: f32, new: f32) -> bool {
    let diff = (old - new).abs();
    let scale = old.abs().max(new.abs()).max(1.0);

    diff / scale > 0.001
}

pub fn vec_changed(old: Vec2, new: Vec2) -> bool {
    (old - new).length_sq() > 1.0
}

pub fn pos_changed(old: Pos2, new: Pos2) -> bool {
    (old - new).length_sq() > 1.0
}
