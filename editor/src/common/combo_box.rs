use eframe::egui;
use egui::{Align2, Color32, FontId, Popup, Pos2, Response, Sense, StrokeKind, Vec2, vec2};

use crate::gui::Gui;
use crate::gui::style::DragValueStyle;

#[derive(Debug)]
pub struct ComboBox<'a> {
    selected: &'a mut String,
    options: &'a [String],
    font: Option<FontId>,
    color: Option<Color32>,
    style: Option<DragValueStyle>,
    padding: Option<Vec2>,
    pos: Pos2,
    align: Align2,
}

impl<'a> ComboBox<'a> {
    pub fn new(selected: &'a mut String, options: &'a [String]) -> Self {
        Self {
            selected,
            options,
            font: None,
            color: None,
            style: None,
            padding: None,
            pos: Pos2::ZERO,
            align: Align2::CENTER_CENTER,
        }
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
        self.style = Some(style);
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
        let font = self.font.unwrap_or_else(|| gui.style.mono_font.clone());
        let color = self.color.unwrap_or(gui.style.text_color);
        let padding = self
            .padding
            .unwrap_or_else(|| vec2(gui.style.small_padding, 0.0));
        let style = self
            .style
            .unwrap_or_else(|| gui.style.node.const_bind_style.clone());

        let galley = gui
            .ui()
            .painter()
            .layout_no_wrap(self.selected.clone(), font.clone(), color);
        let size = galley.size() + padding * 2.0;

        let rect = self.align.anchor_size(self.pos, size);
        let inner_rect = rect.shrink2(padding);

        if !gui.ui().is_rect_visible(rect) {
            return gui.ui().allocate_rect(rect, Sense::hover());
        }

        let id = gui.ui().make_persistent_id(id_salt);
        let popup_id = id.with("popup");

        gui.painter().rect(
            rect,
            style.radius,
            style.fill,
            style.stroke,
            StrokeKind::Outside,
        );

        let text_anchor = self.align.pos_in_rect(&inner_rect);
        let text_rect = self.align.anchor_size(text_anchor, galley.size());
        gui.painter().galley(text_rect.min, galley, color);

        let mut response = gui.ui().allocate_rect(rect, Sense::click());

        if response.clicked() {
            Popup::toggle_id(gui.ui().ctx(), popup_id);
        }

        let mut selected_option: Option<String> = None;
        let is_open = Popup::is_id_open(gui.ui().ctx(), popup_id);

        if is_open {
            let popup_pos = rect.left_bottom();
            egui::Area::new(popup_id)
                .order(egui::Order::Foreground)
                .fixed_pos(popup_pos)
                .show(gui.ui().ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        for option in self.options {
                            let is_selected = option == self.selected;
                            if ui.selectable_label(is_selected, option).clicked() {
                                selected_option = Some(option.clone());
                                Popup::close_id(ui.ctx(), popup_id);
                            }
                        }
                    });
                });
        }

        if let Some(new_selection) = selected_option {
            *self.selected = new_selection;
            response.mark_changed();
        }

        response
    }
}
