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

        // Check if we're currently dragging to display temporary value
        let id = gui.ui().make_persistent_id(&id_salt);
        let drag_temp_id = id.with("drag_temp");
        let display_value = gui
            .ui()
            .data_mut(|data| data.get_temp::<i64>(drag_temp_id))
            .unwrap_or(*self.value);

        let value_text = display_value.to_string();
        let galley = gui
            .ui()
            .painter()
            .layout_no_wrap(value_text.clone(), font.clone(), color);
        let mut size = galley.size() + padding * 2.0; //+ vec2(0.0, 4.0 * gui.scale());
        size.x = size.x.max(30.0 * gui.scale());
        assert!(size.x.is_finite() && size.y.is_finite());

        let rect = self.align.anchor_size(self.pos, size);
        let inner_rect = rect.shrink2(padding);

        if !gui.ui().is_rect_visible(rect) {
            return gui.ui().allocate_rect(rect, Sense::hover());
        }

        let id = gui.ui().make_persistent_id(id_salt);
        let edit_id = id.with("edit");
        let edit_text_id = id.with("edit_text");
        let edit_original_id = id.with("edit_original");
        let drag_temp_id = id.with("drag_temp");
        let mut edit_active = gui
            .ui()
            .data_mut(|data| data.get_temp::<bool>(edit_id))
            .unwrap_or(false);

        gui.painter().rect(
            rect,
            background.radius,
            background.fill,
            background.stroke,
            StrokeKind::Outside,
        );
        // gui.painter().rect(
        //     inner_rect,
        //     0.0,
        //     Color32::TRANSPARENT,
        //     gui.style.active_bg_stroke,
        //     StrokeKind::Middle,
        // );

        if edit_active {
            let mut edit_text = gui
                .ui()
                .data_mut(|data| data.get_temp::<String>(edit_text_id))
                .unwrap_or_else(|| self.value.to_string());
            let original_value = gui
                .ui()
                .data_mut(|data| data.get_temp::<i64>(edit_original_id))
                .unwrap_or(*self.value);

            let text_edit = TextEdit::singleline(&mut edit_text)
                .id(edit_id)
                .font(font.clone())
                .desired_width(inner_rect.width())
                .horizontal_align(self.align.x())
                .vertical_align(self.align.y())
                .clip_text(true)
                .frame(false);
            let mut text_edit_response = gui.ui().put(inner_rect, text_edit);

            // gui.painter().rect(
            //     text_edit_response.rect,
            //     0.0,
            //     Color32::TRANSPARENT,
            //     gui.style.active_bg_stroke,
            //     StrokeKind::Middle,
            // );

            let should_confirm = text_edit_response.lost_focus()
                && gui
                    .ui()
                    .input(|input| input.key_pressed(Key::Enter) || input.pointer.any_click());
            let should_cancel = text_edit_response.has_focus()
                && gui.ui().input(|input| input.key_pressed(Key::Escape));

            let mut value_actually_changed = false;
            if should_confirm {
                if let Ok(parsed) = edit_text.trim().parse::<i64>()
                    && parsed != original_value
                {
                    *self.value = parsed;
                    value_actually_changed = true;
                }
                edit_active = false;
            } else if should_cancel {
                edit_active = false;
            }

            gui.ui().data_mut(|data| {
                if edit_active {
                    data.insert_temp(edit_id, true);
                    data.insert_temp(edit_text_id, edit_text);
                    data.insert_temp(edit_original_id, original_value);
                } else {
                    data.remove::<bool>(edit_id);
                    data.remove::<String>(edit_text_id);
                    data.remove::<i64>(edit_original_id);
                }
            });

            // We need to return the text_edit_response to preserve editing functionality,
            // but TextEdit marks it as changed() when text is typed. Unfortunately we can't
            // easily clear the changed flag. The best we can do is only explicitly mark
            // as changed when value is committed, but changed() might still be true while typing.
            // A better solution would be to track edit state externally in the caller.
            if value_actually_changed {
                text_edit_response.mark_changed();
            }

            return text_edit_response;
        }

        let mut response = gui
            .ui()
            .allocate_rect(inner_rect, Sense::click_and_drag() | Sense::hover());

        if response.clicked() {
            gui.ui().data_mut(|data| {
                data.insert_temp(edit_id, true);
                data.insert_temp(edit_text_id, self.value.to_string());
                data.insert_temp(edit_original_id, *self.value);
            });
            gui.ui().memory_mut(|memory| memory.request_focus(edit_id));
        }

        if response.drag_started() {
            gui.ui().data_mut(|data| {
                data.insert_temp(id, *self.value);
                data.insert_temp(drag_temp_id, *self.value);
            });
        }

        if response.dragged() {
            let start_value = gui
                .ui()
                .data_mut(|data| data.get_temp::<i64>(id))
                .unwrap_or(*self.value);
            let delta = response
                .total_drag_delta()
                .expect("dragged response should have total delta")
                .x;
            let new_value = start_value + (delta * self.speed).round() as i64;
            let current_temp = gui
                .ui()
                .data_mut(|data| data.get_temp::<i64>(drag_temp_id))
                .unwrap_or(*self.value);
            if new_value != current_temp {
                gui.ui()
                    .data_mut(|data| data.insert_temp(drag_temp_id, new_value));
            }
        }

        if response.drag_stopped() {
            let final_value = gui
                .ui()
                .data_mut(|data| data.get_temp::<i64>(drag_temp_id))
                .unwrap_or(*self.value);
            if final_value != *self.value {
                *self.value = final_value;
                response.mark_changed();
            }
            gui.ui().data_mut(|data| {
                data.remove::<i64>(id);
                data.remove::<i64>(drag_temp_id);
            });
        }

        let text_anchor = self.align.pos_in_rect(&inner_rect);
        let text_rect = self.align.anchor_size(text_anchor, galley.size());
        gui.painter().galley(text_rect.min, galley, color);

        response
    }
}
