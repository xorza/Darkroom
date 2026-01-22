use std::sync::Arc;

use egui::{Align, FontId, Galley, Id, Key, Order, Pos2, Response, Sense, Vec2, vec2};

use crate::common::area::Area;
use crate::common::button::Button;
use crate::common::frame::Frame;
use crate::gui::Gui;
use crate::gui::style::{ButtonStyle, PopupStyle};

/// A custom popup menu that works with the Gui struct.
/// Opens on click of the anchor response and closes on click outside or item selection.
/// Styled like new_node_ui popup.
#[derive(Debug)]
pub struct PopupMenu {
    id: Id,
    anchor_response: Response,
    style: Option<PopupStyle>,
    close_on_click: bool,
    min_width: Option<f32>,
}

impl PopupMenu {
    /// Create a new popup menu anchored to the given response.
    /// The popup will open when the response is clicked.
    pub fn new(anchor_response: &Response, id_salt: impl std::hash::Hash) -> Self {
        let id = anchor_response.id.with(id_salt);

        // Toggle popup on click using egui memory (like new_node_ui does)
        if anchor_response.clicked() {
            let ctx = &anchor_response.ctx;
            let is_open = ctx.memory(|mem| mem.data.get_temp::<bool>(id).unwrap_or(false));
            ctx.memory_mut(|mem| mem.data.insert_temp(id, !is_open));
        }

        Self {
            id,
            anchor_response: anchor_response.clone(),
            style: None,
            close_on_click: true,
            min_width: None,
        }
    }

    pub fn style(mut self, style: PopupStyle) -> Self {
        self.style = Some(style);
        self
    }

    pub fn close_on_click(mut self, close: bool) -> Self {
        self.close_on_click = close;
        self
    }

    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = Some(width);
        self
    }

    /// Show the popup menu if it's open.
    /// Returns Some(inner) if the popup was shown, None otherwise.
    pub fn show<R>(self, gui: &mut Gui<'_>, content: impl FnOnce(&mut Gui<'_>) -> R) -> Option<R> {
        let ctx = gui.ui().ctx().clone();
        let is_open = ctx.memory(|mem| mem.data.get_temp::<bool>(self.id).unwrap_or(false));

        if !is_open {
            return None;
        }

        let popup_style = self.style.unwrap_or_else(|| gui.style.popup.clone());
        let close_on_click = self.close_on_click;
        let anchor_rect = self.anchor_response.rect;
        let min_width = self.min_width;
        let popup_id = self.id;
        // Check if the popup was just opened on this frame (anchor was clicked)
        let just_opened = self.anchor_response.clicked();

        let popup_response = Area::new(self.id)
            .fixed_pos(anchor_rect.left_bottom())
            .order(Order::Foreground)
            .show(gui, |gui| {
                Frame::popup(&popup_style).show(gui, |gui| {
                    if let Some(width) = min_width {
                        gui.ui().set_min_width(width);
                    }
                    content(gui)
                })
            });

        let inner = popup_response.inner.inner;
        let popup_rect = popup_response.response.rect;

        // Don't process close events on the frame the popup was just opened
        if just_opened {
            return Some(inner);
        }

        // Close on click outside
        if gui.ui().input(|i| i.pointer.any_pressed())
            && let Some(pointer_pos) = gui.ui().input(|i| i.pointer.interact_pos())
            && !popup_rect.contains(pointer_pos)
            && !anchor_rect.contains(pointer_pos)
        {
            ctx.memory_mut(|mem| mem.data.insert_temp(popup_id, false));
        }

        // Close on click inside if close_on_click is true
        // But not if clicking on the anchor (which toggles the popup)
        if close_on_click
            && gui.ui().input(|i| i.pointer.any_click())
            && let Some(pointer_pos) = gui.ui().input(|i| i.pointer.interact_pos())
            && popup_rect.contains(pointer_pos)
            && !anchor_rect.contains(pointer_pos)
        {
            ctx.memory_mut(|mem| mem.data.insert_temp(popup_id, false));
        }

        // Close on Escape
        if gui.ui().input(|i| i.key_pressed(Key::Escape)) {
            ctx.memory_mut(|mem| mem.data.insert_temp(popup_id, false));
        }

        Some(inner)
    }
}

/// A list item widget for use inside PopupMenu or other list contexts.
/// Uses Button internally to ensure consistent styling with new_node_ui.
pub struct ListItem<'a> {
    text: Option<&'a str>,
    galley: Option<Arc<Galley>>,
    selected: bool,
    enabled: bool,
    font: Option<FontId>,
    style: Option<ButtonStyle>,
    size: Option<Vec2>,
    align: Align,
    tooltip: Option<&'a str>,
}

impl<'a> ListItem<'a> {
    pub fn from_str(text: &'a str) -> Self {
        Self {
            text: Some(text),
            galley: None,
            selected: false,
            enabled: true,
            font: None,
            style: None,
            size: None,
            align: Align::Min,
            tooltip: None,
        }
    }

    /// Create a ListItem with a pre-computed galley (for performance with ColumnFlow).
    pub fn from_galley(galley: Arc<Galley>) -> Self {
        Self {
            text: None,
            galley: Some(galley),
            selected: false,
            enabled: true,
            font: None,
            style: None,
            size: None,
            align: Align::Min,
            tooltip: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn font(mut self, font: FontId) -> Self {
        self.font = Some(font);
        self
    }

    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.style = Some(style);
        self
    }

    pub fn size(mut self, size: Vec2) -> Self {
        self.size = Some(size);
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    pub fn tooltip(mut self, tooltip: &'a str) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        let style = self.style.unwrap_or(gui.style.list_button);

        // Calculate size like new_node_ui does
        let size = if let Some(size) = self.size {
            size
        } else {
            let font = self
                .font
                .clone()
                .unwrap_or_else(|| gui.style.sub_font.clone());
            let padding = gui.style.padding;
            let small_padding = gui.style.small_padding;

            let text_width = if let Some(galley) = &self.galley {
                galley.size().x
            } else {
                let galley = gui.painter().layout_no_wrap(
                    self.text.unwrap_or("").to_string(),
                    font.clone(),
                    gui.style.text_color,
                );
                galley.size().x
            };

            let width = text_width + padding * 2.0;
            let height = gui.font_height(&font) + small_padding * 2.0;
            vec2(width, height)
        };

        let mut selected = self.selected;

        let mut btn = Button::default()
            .background(style)
            .enabled(self.enabled)
            .align(self.align)
            .size(size)
            .toggle(&mut selected);

        // Set text or galley
        if let Some(galley) = self.galley {
            btn = btn.galley(galley);
        } else if let Some(text) = self.text {
            btn = btn.text(text);
        }

        if let Some(font) = self.font {
            btn = btn.font(font);
        }

        if let Some(tooltip) = self.tooltip {
            btn = btn.tooltip(tooltip);
        }

        btn.show(gui)
    }
}
