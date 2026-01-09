use eframe::egui;
use egui::{
    Align, Align2, Color32, CursorIcon, FontId, Key, Pos2, Response, Sense, Stroke, StrokeKind,
    TextEdit, Vec2, vec2,
};

use crate::gui::{Gui, style::DragValueStyle};

#[derive(Debug)]
pub struct DragValue<'a> {
    value: &'a mut i64,
    speed: f32,
    font: Option<FontId>,
    color: Option<Color32>,
    background: Option<DragValueStyle>,
    padding: Option<Vec2>,
    pos: Pos2,
    align: Align2,
    hover: bool,
}

impl<'a> DragValue<'a> {
    pub fn new(value: &'a mut i64) -> Self {
        Self {
            value,
            speed: 1.0,
            font: None,
            color: None,
            background: None,
            padding: None,
            pos: Pos2::ZERO,
            align: Align2::CENTER_CENTER,
            hover: true,
        }
    }

    pub fn speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    pub fn font(mut self, font: FontId) -> Self {
        self.font = Some(font);
        self
    }

    pub fn color(mut self, color: Color32) -> Self {
        self.color = Some(color);
        self
    }

    pub fn style(mut self, style: DragValueStyle) -> Self {
        assert!(style.radius.is_finite());
        self.background = Some(style);
        self
    }

    pub fn padding(mut self, padding: Vec2) -> Self {
        assert!(padding.x.is_finite() && padding.y.is_finite());
        assert!(padding.x >= 0.0 && padding.y >= 0.0);
        self.padding = Some(padding);
        self
    }

    pub fn pos(mut self, pos: Pos2) -> Self {
        assert!(pos.x.is_finite() && pos.y.is_finite());
        self.pos = pos;
        self
    }

    pub fn align(mut self, align: Align2) -> Self {
        self.align = align;
        self
    }

    pub fn show(self, gui: &mut Gui<'_>, id_salt: impl std::hash::Hash) -> Response {
        assert!(self.speed.is_finite());

        let font = self.font.unwrap_or_else(|| gui.style.mono_font.clone());
        let color = self.color.unwrap_or(gui.style.text_color);
        let padding = self
            .padding
            .unwrap_or_else(|| Vec2::splat(gui.style.small_padding));
        assert!(padding.x.is_finite() && padding.y.is_finite());
        assert!(padding.x >= 0.0 && padding.y >= 0.0);
        let background = self
            .background
            .unwrap_or(gui.style.node.const_bind_style.clone());
        assert!(background.radius.is_finite());

        let ui = gui.ui();
        let value_text = self.value.to_string();
        let galley = ui
            .painter()
            .layout_no_wrap(value_text.clone(), font.clone(), color);
        let size = galley.size() + padding * 2.0;
        assert!(size.x.is_finite() && size.y.is_finite());

        let mut rect = self.align.anchor_size(self.pos, size);

        let id = ui.make_persistent_id(id_salt);
        let edit_id = id.with("edit");
        let edit_text_id = id.with("edit_text");
        let mut edit_active = ui
            .data_mut(|data| data.get_temp::<bool>(edit_id))
            .unwrap_or(false);

        ui.painter().rect(
            rect,
            background.radius,
            background.fill,
            background.stroke,
            StrokeKind::Outside,
        );

        rect = rect.expand(background.stroke.width);

        if edit_active {
            let mut edit_text = ui
                .data_mut(|data| data.get_temp::<String>(edit_text_id))
                .unwrap_or_else(|| self.value.to_string());
            let text_edit = TextEdit::singleline(&mut edit_text)
                .id(edit_id)
                .font(font.clone())
                .desired_width(rect.width())
                .vertical_align(Align::Center)
                .horizontal_align(self.align.x())
                .margin(padding)
                .clip_text(true)
                .frame(false);
            let mut response = ui.put(rect, text_edit);

            if response.lost_focus()
                && ui.input(|input| input.key_pressed(Key::Enter) || input.pointer.any_click())
            {
                if let Ok(parsed) = edit_text.trim().parse::<i64>()
                    && parsed != *self.value
                {
                    *self.value = parsed;
                    response.mark_changed();
                }
                edit_active = false;
            } else if response.has_focus() && ui.input(|input| input.key_pressed(Key::Escape)) {
                edit_active = false;
            }

            ui.data_mut(|data| {
                if edit_active {
                    data.insert_temp(edit_id, true);
                    data.insert_temp(edit_text_id, edit_text);
                } else {
                    data.remove::<bool>(edit_id);
                    data.remove::<String>(edit_text_id);
                }
            });

            return response;
        }

        let mut response = ui.allocate_rect(rect, Sense::click_and_drag() | Sense::hover());

        if response.clicked() {
            ui.data_mut(|data| {
                data.insert_temp(edit_id, true);
                data.insert_temp(edit_text_id, self.value.to_string());
            });
            ui.memory_mut(|memory| memory.request_focus(edit_id));
        }

        if response.drag_started() {
            ui.data_mut(|data| data.insert_temp(id, *self.value));
        }

        if response.dragged() {
            let start_value = ui
                .data_mut(|data| data.get_temp::<i64>(id))
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
            ui.data_mut(|data| data.remove::<i64>(id));
        }

        let inner_rect = rect.shrink2(padding);
        let text_anchor = self.align.pos_in_rect(&inner_rect);
        let text_rect = self.align.anchor_size(text_anchor, galley.size());
        ui.painter().galley(text_rect.min, galley, color);

        response
    }
}
