use std::hash::Hash;
use std::sync::Arc;

use egui::{Align, FontId, Galley, Response, Vec2, vec2};

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::style::{ButtonStyle, PopupStyle};
use crate::gui::widgets::button::Button;

/// Click-anchored popup menu styled with our [`PopupStyle`]. Thin
/// wrapper around [`egui::Popup::menu`] — open/close lifecycle, click
/// outside, escape, and the "just-opened" race are all handled by
/// stock egui.
#[derive(Debug)]
#[must_use = "PopupMenu does nothing until .show() is called"]
pub struct PopupMenu<'a> {
    anchor: &'a Response,
    id: egui::Id,
    style: Option<PopupStyle>,
    min_width: Option<f32>,
}

impl<'a> PopupMenu<'a> {
    pub fn new(anchor: &'a Response, id_salt: impl Hash) -> Self {
        Self {
            anchor,
            id: anchor.id.with(id_salt),
            style: None,
            min_width: None,
        }
    }

    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = Some(width);
        self
    }

    /// Returns `Some(inner)` when the popup is open this frame, `None`
    /// when closed.
    pub fn show<R>(self, gui: &mut Gui<'_>, content: impl FnOnce(&mut Gui<'_>) -> R) -> Option<R> {
        let style = self.style.unwrap_or_else(|| gui.style.popup.clone());
        let frame = egui::Frame::NONE
            .fill(style.fill)
            .stroke(style.stroke)
            .corner_radius(style.corner_radius)
            .inner_margin(style.padding);

        let mut popup = egui::Popup::menu(self.anchor).id(self.id).frame(frame);
        if let Some(w) = self.min_width {
            popup = popup.width(w);
        }

        let args = gui.child_args();
        popup
            .show(|ui| args.enter(ui, content))
            .map(|inner| inner.inner)
    }
}

/// A list item widget for use inside [`PopupMenu`] or other list
/// contexts. Wraps [`Button`] with default sizing tied to the active
/// font and `style.list_button` styling.
#[must_use = "ListItem does nothing until .show() is called"]
pub struct ListItem<'a> {
    id: StableId,
    text: Option<&'a str>,
    galley: Option<Arc<Galley>>,
    selected: bool,
    font: Option<FontId>,
    style: Option<ButtonStyle>,
    size: Option<Vec2>,
    tooltip: Option<&'a str>,
}

impl<'a> ListItem<'a> {
    pub fn from_str(id: StableId, text: &'a str) -> Self {
        Self {
            id,
            text: Some(text),
            galley: None,
            selected: false,
            font: None,
            style: None,
            size: None,
            tooltip: None,
        }
    }

    /// Create a ListItem with a pre-computed galley (for performance with ColumnFlow).
    pub fn from_galley(id: StableId, galley: Arc<Galley>) -> Self {
        Self {
            id,
            text: None,
            galley: Some(galley),
            selected: false,
            font: None,
            style: None,
            size: None,
            tooltip: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
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

    pub fn tooltip(mut self, tooltip: &'a str) -> Self {
        self.tooltip = Some(tooltip);
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        let style = self.style.unwrap_or(gui.style.list_button);
        let id = self.id;

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

        let mut btn = Button::new(id)
            .background(style)
            .text_align(Align::Min)
            .size(size)
            .toggle(&mut selected);

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
