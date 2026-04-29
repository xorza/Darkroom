use std::sync::Arc;

use eframe::egui;
use egui::{Align, FontId, Galley, Rect, Response, Sense, StrokeKind, Vec2, vec2};

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::style::ButtonStyle;
use crate::gui::widgets::HitRegion;

#[must_use = "Button does nothing until .show() is called"]
pub struct Button<'a> {
    id: StableId,
    enabled: bool,
    font: Option<FontId>,
    tooltip: Option<&'a str>,
    background: Option<ButtonStyle>,
    rect: Option<Rect>,
    size: Option<Vec2>,
    /// Per-side padding between the button's outer rect and its
    /// content (text / shapes). Defaults to
    /// `vec2(style.padding, style.small_padding)`. Ignored when
    /// `.rect(..)` or `.size(..)` pin the outer rect explicitly.
    padding: Option<Vec2>,
    toggle_value: Option<&'a mut bool>,
    content_align: Align,

    text: Option<&'a str>,
    custom_galley: Option<Arc<Galley>>,
}

impl<'a> Button<'a> {
    /// Construct a button. `id` pins the button's widget identity; see
    /// [`StableId`] for why this shields it from egui's counter-based
    /// auto-id drift.
    pub fn new(id: StableId) -> Self {
        Self {
            id,
            enabled: true,
            text: None,
            font: None,
            tooltip: None,
            background: None,
            rect: None,
            size: None,
            padding: None,
            toggle_value: None,
            content_align: Align::Center,
            custom_galley: None,
        }
    }

    /// No live caller, but the disabled-state rendering paths in `show`
    /// (greyed fill, no-click sense, muted text color) all still react
    /// to this flag — keeping the setter avoids re-deriving them when
    /// the next menu/button needs a disabled state.
    #[allow(dead_code)]
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

    /// Per-side padding between the outer rect and the button's text /
    /// shapes. Used only in autosize mode (no `.rect` / `.size` set).
    pub fn padding(mut self, padding: Vec2) -> Self {
        self.padding = Some(padding);
        self
    }

    pub fn tooltip(mut self, tooltip: &'a str) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    pub fn background(mut self, background: ButtonStyle) -> Self {
        assert!(background.radius.is_finite());
        self.background = Some(background);
        self
    }

    pub fn rect(mut self, rect: Rect) -> Self {
        self.rect = Some(rect);
        self
    }

    pub fn toggle(mut self, value: &'a mut bool) -> Self {
        self.toggle_value = Some(value);
        self
    }

    pub fn text_align(mut self, align: Align) -> Self {
        self.content_align = align;
        self
    }

    pub fn galley(mut self, galley: Arc<Galley>) -> Self {
        self.custom_galley = Some(galley);
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        let id = self.id;
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
            Some(gui.layout_no_wrap(text, &font, text_color))
        } else {
            self.custom_galley.as_ref().map(|g| g.clone())
        };

        let sense = if self.enabled {
            Sense::click() | Sense::hover()
        } else {
            Sense::hover()
        };

        // Two layout modes:
        // - Explicit rect: route through HitRegion::interact_and_cull
        //   so visibility culling and the single-interact registration
        //   share a code path with the rest of the positioned-widget
        //   family.
        // - Autosize: route through ScopedGui::autosize so the
        //   `allocate_*` plumbing stays behind one whitelisted helper.
        let (rect, response) = if let Some(rect) = self.rect {
            let out = HitRegion::new(id)
                .rect(rect)
                .sense(sense)
                .interact_and_cull(gui);
            if !out.visible {
                return out.response;
            }
            (out.rect, out.response)
        } else {
            // todo also include provided shapes size
            let text_size = galley.as_ref().map(|g| g.size()).unwrap_or_default();
            // Per-side padding; doubled to get the outer rect width.
            // Height locks to `style.row_height` so buttons line up
            // with text edits and other row-height widgets in form
            // layouts; the padding.y component is not used in autosize.
            let padding = self
                .padding
                .unwrap_or_else(|| vec2(gui.style.padding, gui.style.small_padding));
            let autosize = self
                .size
                .unwrap_or_else(|| vec2(text_size.x + padding.x * 2.0, gui.style.row_height));
            let (rect, response) = gui.scope(id).autosize(autosize, sense);
            if !gui.ui_raw().is_rect_visible(rect) {
                return response;
            }
            (rect, response)
        };

        if response.clicked()
            && self.enabled
            && let Some(toggle_value) = self.toggle_value
        {
            *toggle_value = !*toggle_value;
        }

        let response = match self.tooltip {
            Some(tooltip) if !tooltip.is_empty() => response.on_hover_text(tooltip),
            _ => response,
        };

        let background = self.background.unwrap_or(ButtonStyle {
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
