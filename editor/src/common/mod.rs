use egui::Vec2;

pub mod connection_bezier;
pub mod font;

pub fn scale_changed(old: f32, new: f32) -> bool {
    (old / new - 1.0).abs() > 0.01
}

pub fn pan_changed(old: Vec2, new: Vec2) -> bool {
    (old - new).length_sq() > 1.0
}
