use eframe::egui;
use egui::{Align, Color32, FontId, Rect, Response, Sense, Shape, Stroke, StrokeKind, Vec2, vec2};

use crate::gui::Gui;

#[derive(Debug, Clone, Copy)]
pub struct ButtonBackground {
    pub disabled_fill: Color32,
    pub idle_fill: Color32,
    pub hover_fill: Color32,
    pub active_fill: Color32,
    pub checked_fill: Color32,
    pub inactive_stroke: Stroke,
    pub hovered_stroke: Stroke,
    pub radius: f32,
}

#[derive(Debug)]
pub struct Button<'a> {
    enabled: bool,
    text: Option<&'a str>,
    font: Option<FontId>,
    tooltip: Option<&'a str>,
    background: Option<ButtonBackground>,
    rect: Option<Rect>,
    size: Option<Vec2>,
    shapes: Vec<Shape>,
    toggle_value: Option<&'a mut bool>,
    content_align: Align,
}

impl<'a> Default for Button<'a> {
    fn default() -> Self {
        Self {
            enabled: true,
            text: None,
            font: None,
            tooltip: None,
            background: None,
            rect: None,
            size: None,
            shapes: Vec::new(),
            toggle_value: None,
            content_align: Align::Center,
        }
    }
}

impl<'a> Button<'a> {
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn text(mut self, text: &'a str) -> Self {
        self.text = Some(text);
        self
    }

    pub fn font(mut self, font: FontId) -> Self {
        self.font = Some(font);
        self
    }

    pub fn size(mut self, size: Vec2) -> Self {
        self.size = Some(size);
        self
    }

    pub fn tooltip(mut self, tooltip: &'a str) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    pub fn background(mut self, background: ButtonBackground) -> Self {
        assert!(background.radius.is_finite());
        self.background = Some(background);
        self
    }

    pub fn rect(mut self, rect: Rect) -> Self {
        self.rect = Some(rect);
        self
    }

    pub fn shapes(mut self, shapes: impl IntoIterator<Item = impl Into<Shape>>) -> Self {
        self.shapes = shapes.into_iter().map(Into::into).collect();
        self
    }

    pub fn toggle(mut self, value: &'a mut bool) -> Self {
        self.toggle_value = Some(value);
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.content_align = align;
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        let is_checked = self.toggle_value.as_deref().copied().unwrap_or(false);

        let text_color = if !self.enabled {
            gui.style.noninteractive_text_color
        } else if is_checked {
            gui.style.dark_text_color
        } else {
            gui.style.text_color
        };
        let galley = if let Some(text) = self.text {
            let font = self.font.unwrap_or_else(|| gui.style.sub_font.clone());
            let galley = gui
                .painter()
                .layout_no_wrap(text.to_string(), font, text_color);
            Some(galley)
        } else {
            None
        };

        let sense = if self.enabled {
            Sense::click() | Sense::hover()
        } else {
            Sense::hover()
        };

        let (rect, response) = if let Some(rect) = self.rect {
            let response = gui.ui().allocate_rect(rect, sense);
            (rect, response)
        } else if let Some(size) = self.size {
            gui.ui().allocate_exact_size(size, sense)
        } else {
            // Autosize: calculate button size based on text
            // todo also include provided shapes size
            let text_size = galley.as_ref().map(|g| g.size()).unwrap_or_default();
            let padding = vec2(gui.style.padding * 2.0, gui.style.small_padding * 2.0);
            let button_size = text_size + padding;
            gui.ui().allocate_exact_size(button_size, sense)
        };

        if !gui.ui().is_rect_visible(rect) {
            return response;
        }

        if response.clicked()
            && self.enabled
            && let Some(toggle_value) = self.toggle_value
        {
            *toggle_value = !*toggle_value;
        }

        if response.hovered()
            && let Some(tooltip) = self.tooltip
            && !tooltip.is_empty()
        {
            response.show_tooltip_text(tooltip);
        }

        let background = self.background.unwrap_or(ButtonBackground {
            disabled_fill: gui.style.noninteractive_bg_fill,
            idle_fill: gui.style.inactive_bg_fill,
            hover_fill: gui.style.hover_bg_fill,
            active_fill: gui.style.active_bg_fill,
            checked_fill: gui.style.checked_bg_fill,
            inactive_stroke: gui.style.inactive_bg_stroke,
            hovered_stroke: gui.style.active_bg_stroke,
            radius: gui.style.small_corner_radius,
        });

        let fill = if !self.enabled {
            background.disabled_fill
        } else if is_checked {
            background.checked_fill
        } else if response.is_pointer_button_down_on() {
            background.active_fill
        } else if response.hovered() {
            background.hover_fill
        } else {
            background.idle_fill
        };

        let stroke = if response.hovered() && self.enabled {
            background.hovered_stroke
        } else {
            background.inactive_stroke
        };

        gui.painter()
            .rect(rect, background.radius, fill, stroke, StrokeKind::Inside);

        if !self.shapes.is_empty() {
            gui.painter().extend(self.shapes);
        }
        if let Some(galley) = galley {
            let text_x = match self.content_align {
                Align::Min => rect.min.x + gui.style.padding,
                Align::Center => rect.min.x + (rect.width() - galley.size().x) * 0.5,
                Align::Max => rect.max.x - galley.size().x - gui.style.padding,
            };
            let text_y = rect.min.y + (rect.height() - galley.size().y) * 0.5;
            gui.painter()
                .galley(egui::pos2(text_x, text_y), galley, text_color);
        }

        response
    }
}
