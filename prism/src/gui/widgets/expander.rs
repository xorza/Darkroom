use std::f32::consts::TAU;

use egui::{Color32, InnerResponse, Painter, Pos2, Rect, Sense, Shape, Stroke, Vec2, vec2};

use crate::common::StableId;
use crate::gui::Gui;

#[derive(Debug)]
#[must_use = "Expander does nothing until .show() is called"]
pub struct Expander {
    id: StableId,
    text: String,
    default_open: bool,
}

impl Expander {
    pub fn new(id: StableId, text: impl Into<String>) -> Self {
        Self {
            id,
            text: text.into(),
            default_open: false,
        }
    }

    pub fn default_open(mut self, default_open: bool) -> Self {
        self.default_open = default_open;
        self
    }

    /// Returns the header [`Response`] (so callers can attach a tooltip
    /// or detect a click) and the body closure's return value. When the
    /// expander is collapsed, the body is not invoked and `inner` is `None`.
    pub fn show<R>(
        self,
        gui: &mut Gui<'_>,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<Option<R>> {
        let mut open = gui.memory().load_persistent(self.id, self.default_open);

        let icon_size = gui.style.body_font.size;
        let icon_spacing = gui.style.padding;
        let text_color = gui.style.text_color;

        let galley = gui.layout_no_wrap(&self.text, &gui.style.body_font, text_color);

        let header_height = galley.size().y.max(icon_size);
        let header_width = icon_size + icon_spacing + galley.size().x;
        let header_size = vec2(header_width, header_height);

        let header_response = gui.scope(self.id).show(|gui| {
            // Allocation runs inside the `Gui::scope` above, so its auto-id
            // seeds from the scope's stable id rather than the parent counter.
            let (header_rect, response) = gui
                .ui_raw()
                .allocate_exact_size(header_size, Sense::click()); // id-drift-ok

            if gui.ui_raw().is_rect_visible(header_rect) {
                let icon_rect = Rect::from_min_size(header_rect.min, Vec2::splat(icon_size));
                paint_icon(gui.painter(), icon_rect, open, text_color);

                let text_pos = Pos2::new(
                    header_rect.min.x + icon_size + icon_spacing,
                    header_rect.min.y + (header_height - galley.size().y) * 0.5,
                );
                gui.painter().galley(text_pos, galley, text_color);
            }

            response
        });

        if header_response.clicked() {
            open = !open;
            gui.memory().store_persistent(self.id, open);
        }

        let inner = open.then(|| add_contents(gui));
        InnerResponse::new(inner, header_response)
    }
}

fn paint_icon(painter: &Painter, rect: Rect, open: bool, color: Color32) {
    let center = rect.center();
    let radius = rect.width() * 0.35;

    let rotation = if open { TAU * 0.5 } else { TAU * 0.25 };

    let points: Vec<Pos2> = (0..3)
        .map(|i| {
            let angle = rotation + TAU * (i as f32) / 3.0 - TAU / 4.0;
            center + radius * Vec2::new(angle.cos(), angle.sin())
        })
        .collect();

    painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
}
