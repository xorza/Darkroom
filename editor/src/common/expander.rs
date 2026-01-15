use std::f32::consts::TAU;

use egui::{Color32, Id, Pos2, Rect, Sense, Shape, Stroke, Vec2, vec2};

use crate::gui::Gui;

#[derive(Debug)]
pub struct Expander {
    id: Id,
    text: String,
    default_open: bool,
}

impl Expander {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            id: Id::new(&text),
            text,
            default_open: false,
        }
    }

    pub fn id(mut self, id: Id) -> Self {
        self.id = id;
        self
    }

    pub fn default_open(mut self, default_open: bool) -> Self {
        self.default_open = default_open;
        self
    }

    pub fn show(self, gui: &mut Gui<'_>, add_contents: impl FnOnce(&mut Gui<'_>)) {
        let id = gui.ui().id().with(self.id);
        let mut open = gui
            .ui()
            .ctx()
            .data_mut(|d| d.get_persisted::<bool>(id).unwrap_or(self.default_open));

        let icon_size = gui.style.body_font.size;
        let icon_spacing = gui.style.padding;
        let text_color = gui.style.text_color;

        let galley = gui.painter().layout_no_wrap(
            self.text.clone(),
            gui.style.body_font.clone(),
            text_color,
        );

        let header_height = galley.size().y.max(icon_size);
        let header_width = icon_size + icon_spacing + galley.size().x;
        let header_size = vec2(header_width, header_height);

        let (header_rect, header_response) =
            gui.ui().allocate_exact_size(header_size, Sense::click());

        if header_response.clicked() {
            open = !open;
            gui.ui().ctx().data_mut(|d| d.insert_persisted(id, open));
        }

        if gui.ui().is_rect_visible(header_rect) {
            let icon_rect = Rect::from_min_size(header_rect.min, Vec2::splat(icon_size));

            paint_icon(gui, icon_rect, open, text_color);

            let text_pos = Pos2::new(
                header_rect.min.x + icon_size + icon_spacing,
                header_rect.min.y + (header_height - galley.size().y) * 0.5,
            );
            gui.painter().galley(text_pos, galley, text_color);
        }

        if open {
            add_contents(gui);
        }
    }
}

fn paint_icon(gui: &Gui<'_>, rect: Rect, open: bool, color: Color32) {
    let center = rect.center();
    let radius = rect.width() * 0.35;

    let rotation = if open { TAU * 0.5 } else { TAU * 0.25 };

    let points: Vec<Pos2> = (0..3)
        .map(|i| {
            let angle = rotation + TAU * (i as f32) / 3.0 - TAU / 4.0;
            center + radius * Vec2::new(angle.cos(), angle.sin())
        })
        .collect();

    gui.painter()
        .add(Shape::convex_polygon(points, color, Stroke::NONE));
}
