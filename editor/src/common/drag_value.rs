use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

#[derive(Debug)]
pub struct DragValue<'a> {
    value: &'a mut i64,
    speed: f32,
    font: FontId,
    color: Color32,
    id: egui::Id,
    background_enabled: bool,
    background_fill: Color32,
    background_stroke: Stroke,
    background_radius: f32,
    padding: Vec2,
}

impl<'a> DragValue<'a> {
    pub fn new(value: &'a mut i64, id: egui::Id, font: FontId, color: Color32) -> Self {
        Self {
            value,
            speed: 1.0,
            font,
            color,
            id,
            background_enabled: false,
            background_fill: Color32::TRANSPARENT,
            background_stroke: Stroke::NONE,
            background_radius: 0.0,
            padding: Vec2::ZERO,
        }
    }

    pub fn speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    pub fn background(mut self, fill: Color32, stroke: Stroke, radius: f32) -> Self {
        assert!(radius.is_finite());
        self.background_enabled = true;
        self.background_fill = fill;
        self.background_stroke = stroke;
        self.background_radius = radius;
        self
    }

    pub fn padding(mut self, padding: Vec2) -> Self {
        assert!(padding.x.is_finite() && padding.y.is_finite());
        assert!(padding.x >= 0.0 && padding.y >= 0.0);
        self.padding = padding;
        self
    }

    pub fn show(self, ui: &mut Ui, pos: Pos2, align: Align2) -> Response {
        assert!(self.speed.is_finite());

        let value_text = self.value.to_string();
        let galley = ui
            .painter()
            .layout_no_wrap(value_text.clone(), self.font.clone(), self.color);
        let size = galley.size() + self.padding * 2.0;
        assert!(size.x.is_finite() && size.y.is_finite());

        let rect = align.anchor_size(pos, size);
        let mut response = ui.allocate_rect(rect, Sense::click_and_drag());

        if self.background_enabled {
            ui.painter().rect(
                rect,
                self.background_radius,
                self.background_fill,
                self.background_stroke,
                StrokeKind::Outside,
            );
        }

        if response.drag_started() {
            ui.data_mut(|data| data.insert_temp(self.id, *self.value));
        }

        if response.dragged() {
            let start_value = ui
                .data_mut(|data| data.get_temp::<i64>(self.id))
                .unwrap_or(*self.value);
            let delta = response
                .total_drag_delta()
                .expect("dragged response should have total delta")
                .x;
            let new_value = start_value + (delta * self.speed).round() as i64;
            if new_value != *self.value {
                *self.value = new_value;
                response.mark_changed();
            }
        }

        if response.drag_stopped() {
            ui.data_mut(|data| data.remove::<i64>(self.id));
        }

        let inner_rect = rect.shrink2(self.padding);
        let text_anchor = align.pos_in_rect(&inner_rect);
        let text_rect = align.anchor_size(text_anchor, galley.size());
        ui.painter().galley(text_rect.min, galley, self.color);

        response
    }
}
