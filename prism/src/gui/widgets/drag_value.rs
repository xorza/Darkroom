use std::fmt::Display;
use std::str::FromStr;

use eframe::egui;
use egui::{Align2, Color32, FontId, Key, Pos2, Response, Sense, StrokeKind, Vec2};

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::style::DragValueStyle;
use crate::gui::widgets::TextEdit;

/// Trait for numeric types that can be used with DragValue.
pub trait DragValueNumeric: Copy + PartialEq + Display + FromStr + Send + Sync + 'static {
    fn from_drag(start: Self, delta: f32, speed: f32) -> Self;
    fn display(&self) -> String {
        self.to_string()
    }
}

/// Widget-internal state machine. Lives in `egui::Memory::data` under
/// one temp key per `DragValue`, replacing four loose keys. Variants
/// are mutually exclusive — dragging and editing can't happen at the
/// same time, which is enforced by the enum rather than by convention.
#[derive(Clone, Debug)]
enum DragValueState<T> {
    Idle,
    Dragging { start: T, current: T },
    Editing { text: String, original: T },
}

impl DragValueNumeric for i64 {
    fn from_drag(start: Self, delta: f32, speed: f32) -> Self {
        start + (delta * speed).round() as i64
    }
}

impl DragValueNumeric for f64 {
    fn from_drag(start: Self, delta: f32, speed: f32) -> Self {
        start + (delta * speed) as f64
    }

    fn display(&self) -> String {
        format!("{:.4}", self)
    }
}

#[derive(Debug)]
#[must_use = "DragValue does nothing until .show() is called"]
pub struct DragValue<'a, T: DragValueNumeric> {
    id: StableId,
    value: &'a mut T,
    speed: f32,
    font: Option<FontId>,
    color: Option<Color32>,
    background: Option<DragValueStyle>,
    padding: Option<Vec2>,
    pos: Pos2,
    anchor: Align2,
}

impl<'a, T: DragValueNumeric> DragValue<'a, T> {
    pub fn new(id: StableId, value: &'a mut T) -> Self {
        Self {
            id,
            value,
            speed: 1.0,
            font: None,
            color: None,
            background: None,
            padding: None,
            pos: Pos2::ZERO,
            anchor: Align2::CENTER_CENTER,
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

    pub fn anchor(mut self, anchor: Align2) -> Self {
        self.anchor = anchor;
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
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

        let id = self.id.id();
        let state_id = id.with("state");
        let edit_id = id.with("edit");
        let state = gui
            .load_temp::<DragValueState<T>>(state_id)
            .unwrap_or(DragValueState::Idle);

        let display_value = match &state {
            DragValueState::Dragging { current, .. } => *current,
            _ => *self.value,
        };

        let value_text = display_value.display();
        let galley = gui
            .ui_raw()
            .painter()
            .layout_no_wrap(value_text.clone(), font.clone(), color);
        let mut size = galley.size() + padding * 2.0;
        size.x = size.x.max(30.0 * gui.scale());
        assert!(size.x.is_finite() && size.y.is_finite());

        let rect = self.anchor.anchor_size(self.pos, size);
        let inner_rect = rect.shrink2(padding);

        if !gui.ui_raw().is_rect_visible(rect) {
            return gui
                .ui_raw()
                .interact(rect, id.with("drag_interact"), Sense::hover());
        }

        gui.painter().rect(
            rect,
            background.radius,
            background.fill,
            background.stroke,
            StrokeKind::Outside,
        );

        if let DragValueState::Editing { text, original } = state {
            return show_editing(
                gui,
                id,
                edit_id,
                inner_rect,
                font,
                self.anchor,
                self.value,
                text,
                original,
            );
        }

        let mut response = gui.ui_raw().interact(
            inner_rect,
            id.with("drag_interact"),
            Sense::click_and_drag() | Sense::hover(),
        );

        // Drive the state machine. Each response check computes the
        // next state; we write it at the bottom of the function.
        let mut next_state = state;

        if response.clicked() {
            next_state = DragValueState::Editing {
                text: self.value.to_string(),
                original: *self.value,
            };
            gui.ui_raw()
                .memory_mut(|memory| memory.request_focus(edit_id));
        }

        if response.drag_started() {
            next_state = DragValueState::Dragging {
                start: *self.value,
                current: *self.value,
            };
        }

        if response.dragged()
            && let DragValueState::Dragging { start, .. } = &next_state
        {
            let delta = response
                .total_drag_delta()
                .expect("dragged response should have total delta")
                .x;
            next_state = DragValueState::Dragging {
                start: *start,
                current: T::from_drag(*start, delta, self.speed),
            };
        }

        if response.drag_stopped() {
            if let DragValueState::Dragging { current, .. } = &next_state
                && *current != *self.value
            {
                *self.value = *current;
                response.mark_changed();
            }
            next_state = DragValueState::Idle;
        }

        write_state(gui, state_id, next_state);

        let text_anchor = self.anchor.pos_in_rect(&inner_rect);
        let text_rect = self.anchor.anchor_size(text_anchor, galley.size());
        gui.painter().galley(text_rect.min, galley, color);

        response
    }
}

#[expect(clippy::too_many_arguments)]
fn show_editing<T: DragValueNumeric>(
    gui: &mut Gui<'_>,
    id: egui::Id,
    edit_id: egui::Id,
    inner_rect: egui::Rect,
    font: FontId,
    anchor: Align2,
    value: &mut T,
    mut text: String,
    original: T,
) -> Response {
    let state_id = id.with("state");
    let text_edit = TextEdit::singleline(&mut text)
        .id(edit_id)
        .font(font)
        .desired_width(inner_rect.width())
        .horizontal_align(anchor.x())
        .vertical_align(anchor.y())
        .clip_text(true)
        .margin(0.0)
        .frame(false);

    let mut text_edit_response = gui
        .scope(StableId::from_egui_id(id.with("drag_value_text")))
        .max_rect(inner_rect)
        .show(|gui| text_edit.show(gui).response);

    let should_confirm = text_edit_response.lost_focus()
        && gui
            .ui_raw()
            .input(|input| input.key_pressed(Key::Enter) || input.pointer.any_click());
    let should_cancel = text_edit_response.has_focus()
        && gui.ui_raw().input(|input| input.key_pressed(Key::Escape));

    let mut value_actually_changed = false;
    let next_state = if should_confirm {
        if let Ok(parsed) = text.trim().parse::<T>()
            && parsed != original
        {
            *value = parsed;
            value_actually_changed = true;
        }
        DragValueState::<T>::Idle
    } else if should_cancel {
        DragValueState::<T>::Idle
    } else {
        DragValueState::Editing { text, original }
    };

    write_state(gui, state_id, next_state);

    // TextEdit marks the response changed() on every keystroke; we
    // only want changed() set when the value actually commits.
    // Clearing egui's bit isn't exposed, so the best we can do is
    // set our own flag on commit — callers that care should check
    // response transitions, not just .changed().
    if value_actually_changed {
        text_edit_response.mark_changed();
    }

    text_edit_response
}

fn write_state<T: DragValueNumeric>(
    gui: &mut Gui<'_>,
    state_id: egui::Id,
    state: DragValueState<T>,
) {
    match state {
        DragValueState::Idle => gui.remove_temp::<DragValueState<T>>(state_id),
        other => gui.store_temp(state_id, other),
    }
}
