use eframe::egui;
use egui::{Color32, FontId, Id, Order, Popup, Pos2, Response, Sense, StrokeKind, Vec2, vec2};

use crate::gui::Gui;
use crate::gui::style::PopupStyle;

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

        let style = self.style.unwrap_or_else(|| gui.style.popup.clone());

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
                egui::Frame::NONE
                    .fill(style.fill)
                    .stroke(style.stroke)
                    .corner_radius(style.corner_radius)
                    .inner_margin(style.padding)
                    .show(ui, |ui| {
                        let mut gui = Gui::new_with_scale(ui, &gui_style, scale);
                        content(&mut gui)
                    })
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

/// A menu item for use inside PopupMenu.
#[derive(Debug)]
pub struct MenuItem<'a> {
    text: &'a str,
    selected: bool,
    enabled: bool,
    font: Option<FontId>,
}

impl<'a> MenuItem<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            selected: false,
            enabled: true,
            font: None,
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

    pub fn show(self, gui: &mut Gui<'_>) -> Response {
        let font = self.font.unwrap_or_else(|| gui.style.sub_font.clone());
        let text_color = if !self.enabled {
            gui.style.noninteractive_text_color
        } else if self.selected {
            gui.style.dark_text_color
        } else {
            gui.style.text_color
        };

        let galley = gui
            .painter()
            .layout_no_wrap(self.text.to_string(), font, text_color);

        let padding = vec2(gui.style.padding, gui.style.small_padding);
        let size = galley.size() + padding * 2.0;

        let sense = if self.enabled {
            Sense::click() | Sense::hover()
        } else {
            Sense::hover()
        };

        let (rect, response) = gui.ui().allocate_exact_size(size, sense);

        if gui.ui().is_rect_visible(rect) {
            let fill = if self.selected {
                gui.style.checked_bg_fill
            } else if response.hovered() && self.enabled {
                gui.style.hover_bg_fill
            } else {
                Color32::TRANSPARENT
            };

            if fill != Color32::TRANSPARENT {
                gui.painter().rect(
                    rect,
                    gui.style.small_corner_radius,
                    fill,
                    egui::Stroke::NONE,
                    StrokeKind::Inside,
                );
            }

            let text_pos = rect.min + padding;
            gui.painter().galley(text_pos, galley, text_color);
        }

        response
    }
}
