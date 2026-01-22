use egui::FontId;

pub trait ScaledFontId {
    fn scaled(&self, scale: f32) -> FontId;
}

impl ScaledFontId for FontId {
    fn scaled(&self, scale: f32) -> FontId {
        FontId {
            size: self.size * scale,
            family: self.family.clone(),
        }
    }
}
