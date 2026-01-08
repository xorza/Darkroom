use egui::{Pos2, Vec2};

pub mod connection_bezier;
pub mod font;

pub fn scale_changed(old: f32, new: f32) -> bool {
    (old / new - 1.0).abs() > 0.01
}

pub fn vec_changed(old: Vec2, new: Vec2) -> bool {
    (old - new).length_sq() > 1.0
}

pub fn pos_changed(old: Pos2, new: Pos2) -> bool {
    (old - new).length_sq() > 1.0
}
