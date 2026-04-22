//! Collapsible single-line status panel. Collapsed: shows the last
//! line of `status`, truncated. Expanded: shows the last `line_count`
//! lines inside a vertical scroll area stuck to the bottom.

use std::f32::consts::TAU;

use egui::{Color32, Margin, Pos2, Rect, Sense, Shape, Stroke, Vec2, Vec2b, vec2};

use common::StrExt;

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::widgets::frame::Frame;

#[derive(Debug)]
pub struct StatusPanel<'a> {
    id: StableId,
    status: &'a str,
    line_count: usize,
    default_open: bool,
}

impl<'a> StatusPanel<'a> {
    pub fn new(id: StableId, status: &'a str) -> Self {
        Self {
            id,
            status,
            line_count: 6,
            default_open: false,
        }
    }

    pub fn line_count(mut self, line_count: usize) -> Self {
        self.line_count = line_count;
        self
    }

    pub fn default_open(mut self, default_open: bool) -> Self {
        self.default_open = default_open;
        self
    }

    pub fn show(self, gui: &mut Gui<'_>) {
        let style = gui.style.clone();
        let line_height = gui.font_height(&style.body_font);
        let toggle_id = self.id.id().with("open");
        let mut open = gui.load_persistent(toggle_id, self.default_open);

        let frame = Frame::none()
            .fill(style.graph_background.bg_color)
            .stroke(style.inactive_bg_stroke)
            .corner_radius(style.corner_radius)
            .outer_margin(Margin {
                left: style.big_padding as i8,
                right: style.big_padding as i8,
                top: 0,
                bottom: style.big_padding as i8,
            })
            .inner_margin(style.corner_radius);

        frame.show(gui, self.id, |gui| {
            let icon_size = style.body_font.size;
            let text_color = style.text_color;

            let ui = gui.ui_raw();
            ui.take_available_width();
            ui.horizontal(|ui| {
                // id-drift-ok: fixed-size icon + anchored rect, drawn
                // inside our already-scoped Frame child.
                let (icon_rect, icon_response) =
                    ui.allocate_exact_size(Vec2::splat(icon_size), Sense::click());
                if icon_response.clicked() {
                    open = !open;
                }
                paint_triangle(ui.painter(), icon_rect, open, text_color);
                ui.expand_to_include_rect(icon_rect);

                if open {
                    let max_height = line_height * self.line_count as f32;
                    ui.set_height(max_height);

                    egui::ScrollArea::vertical()
                        .auto_shrink(Vec2b::TRUE)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.take_available_width();
                            ui.vertical(|ui| {
                                let lines = self.status.line_count().saturating_sub(1) as f32;
                                let spacer = (max_height - lines * line_height).max(0.0);
                                ui.add_space(spacer);
                                ui.label(self.status);
                            });
                        });
                } else {
                    ui.add(egui::Label::new(self.status.last_line()).truncate());
                }
            });
        });

        gui.store_persistent(toggle_id, open);
    }
}

fn paint_triangle(painter: &egui::Painter, rect: Rect, open: bool, color: Color32) {
    let center = rect.center();
    let radius = rect.width() * 0.35;
    let rotation = if open { TAU * 0.5 } else { TAU * 0.25 };
    let points: Vec<Pos2> = (0..3)
        .map(|i| {
            let angle = rotation + TAU * (i as f32) / 3.0 - TAU / 4.0;
            center + radius * vec2(angle.cos(), angle.sin())
        })
        .collect();
    painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
}
