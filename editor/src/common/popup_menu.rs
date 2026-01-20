use eframe::egui;
use egui::{Align, FontId, Id, Order, Popup, Pos2, Response, Vec2};

use crate::common::button::Button;
use crate::common::frame::Frame;
use crate::gui::Gui;
use crate::gui::style::{ButtonStyle, PopupStyle};

/// A custom popup menu that works with the Gui struct.
/// Opens on click of the anchor response and closes on click outside or item selection.
#[derive(Debug)]
pub struct PopupMenu {
    id: Id,
    anchor_response: Response,
    style: Option<PopupStyle>,
    close_on_click: bool,
}

impl PopupMenu {
    /// Create a new popup menu anchored to the given response.
    /// The popup will open when the response is clicked.
    pub fn new(anchor_response: &Response, id_salt: impl std::hash::Hash) -> Self {
        let id = anchor_response.id.with(id_salt);

        // Toggle popup on click
        if anchor_response.clicked() {
            Popup::toggle_id(&anchor_response.ctx, id);
        }

        Self {
            id,
            anchor_response: anchor_response.clone(),
            style: None,
            close_on_click: true,
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

    /// Show the popup menu if it's open.
    /// Returns Some(inner) if the popup was shown, None otherwise.
    pub fn show<R>(self, gui: &mut Gui<'_>, content: impl FnOnce(&mut Gui<'_>) -> R) -> Option<R> {
        if !Popup::is_id_open(gui.ui().ctx(), self.id) {
            return None;
        }

        let popup_style = self.style.unwrap_or_else(|| gui.style.popup.clone());

        let ctx = gui.ui().ctx().clone();
        let popup_id = self.id;
        let close_on_click = self.close_on_click;
        let anchor_rect = self.anchor_response.rect;
        let gui_style = gui.style.clone();
        let scale = gui.scale();

        let area_response = egui::Area::new(self.id)
            .order(Order::Foreground)
            .default_pos(anchor_rect.left_bottom())
            .show(&ctx, |ui| {
                let mut gui = Gui::new_with_scale(ui, &gui_style, scale);
                Frame::popup(&popup_style)
                    .show(&mut gui, |gui| content(gui))
                    .inner
            });

        let inner = area_response.inner;
        let area_response = area_response.response;

        // Close on click outside
        let clicked_outside = ctx.input(|i| i.pointer.any_click())
            && !area_response.contains_pointer()
            && !anchor_rect.contains(
                ctx.input(|i| i.pointer.interact_pos())
                    .unwrap_or(Pos2::ZERO),
            );

        // Close on any click inside if close_on_click is true
        let clicked_inside = close_on_click
            && ctx.input(|i| i.pointer.any_click())
            && area_response.contains_pointer();

        if clicked_outside || clicked_inside {
            Popup::close_id(&ctx, popup_id);
        }

        Some(inner)
    }
}

/// A list item widget for use inside PopupMenu or other list contexts.
/// Uses Button internally to ensure consistent styling with new_node_ui.
#[derive(Debug)]
pub struct ListItem<'a> {
    text: &'a str,
    selected: bool,
    enabled: bool,
    font: Option<FontId>,
    style: Option<ButtonStyle>,
    size: Option<Vec2>,
    align: Align,
}

impl<'a> ListItem<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            selected: false,
            enabled: true,
            font: None,
            style: None,
            size: None,
            align: Align::Min,
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

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        let mut selected = self.selected;

        let mut btn = Button::default()
            .text(self.text)
            .background(self.style.unwrap_or(gui.style.list_button))
            .enabled(self.enabled)
            .align(self.align)
            .toggle(&mut selected);

        if let Some(font) = self.font {
            btn = btn.font(font);
        }

        if let Some(size) = self.size {
            btn = btn.size(size);
        }

        btn.show(gui)
    }
}
