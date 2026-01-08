use eframe::egui;
use egui::{Align2, Color32, FontId, Rect, Response, Sense, Ui};

#[derive(Debug)]
pub struct DragValue<'a> {
    value: &'a mut i64,
    speed: f32,
    font: FontId,
    color: Color32,
    id: egui::Id,
}

impl<'a> DragValue<'a> {
    pub fn new(value: &'a mut i64, id: egui::Id, font: FontId, color: Color32) -> Self {
        Self {
            value,
            speed: 1.0,
            font,
            color,
            id,
        }
    }

    pub fn speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    pub fn show(self, ui: &mut Ui, rect: Rect) -> Response {
        assert!(rect.width().is_finite() && rect.height().is_finite());
        assert!(self.speed.is_finite());

        let mut response = ui.allocate_rect(rect, Sense::click_and_drag());

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

        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            self.value.to_string(),
            self.font,
            self.color,
        );

        response
    }
}
