use egui::{Align2, Color32, FontId, Pos2, Response, Sense, StrokeKind, Vec2, vec2};

use crate::common::popup_menu::{ListItem, PopupMenu};
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

        let mut selected_option: Option<String> = None;
        let selected = self.selected.clone();
        let options = self.options;

        PopupMenu::new(&response, id_salt).show(gui, |gui| {
            // Calculate max width for all items
            let item_font = gui.style.sub_font.clone();
            let padding = gui.style.padding;
            let small_padding = gui.style.small_padding;

            let max_text_width = options
                .iter()
                .map(|option| {
                    let galley = gui.painter().layout_no_wrap(
                        option.clone(),
                        item_font.clone(),
                        gui.style.text_color,
                    );
                    galley.size().x
                })
                .max_by(|a, b| a.partial_cmp(b).unwrap())
                .unwrap_or(0.0);

            let item_width = max_text_width + padding * 2.0;
            let item_height = gui.font_height(&item_font) + small_padding * 2.0;
            let item_size = vec2(item_width, item_height);

            for option in options {
                let is_selected = option == &selected;
                if ListItem::new(option)
                    .selected(is_selected)
                    .size(item_size)
                    .show(gui)
                    .clicked()
                {
                    selected_option = Some(option.clone());
                }
            }
        });

        if let Some(new_selection) = selected_option {
            *self.selected = new_selection;
            response.mark_changed();
        }

        response
    }
}
