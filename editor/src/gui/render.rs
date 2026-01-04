use eframe::egui;
use egui::{Painter, Rect, Ui};

use crate::gui::style::Style;

pub struct RenderContext<'a> {
    pub ui: &'a mut Ui,
    pub painter: Painter,
    pub rect: Rect,
    pub style: Style,
}

impl<'a> RenderContext<'a> {
    pub fn new(ui: &'a mut Ui) -> Self {
        let style = Style::new();
        let rect = ui.available_rect_before_wrap();
        let painter = ui.painter_at(rect);

        Self {
            ui,
            painter,
            rect,
            style,
        }
    }
}
