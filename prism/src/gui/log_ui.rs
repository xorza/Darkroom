use egui::collapsing_header::{CollapsingState, paint_default_icon};
use egui::{Frame, Label, Margin, ScrollArea, Vec2b};

use crate::gui::Gui;
use common::StrExt;

const LINE_COUNT: usize = 6;

#[derive(Debug, Default)]
pub struct LogUi;

impl LogUi {
    pub fn render(&mut self, gui: &mut Gui, status: &str) {
        let body_font = gui.style.body_font.clone();
        let line_height = gui.font_height(&body_font);
        let style = &gui.style;

        let frame = Frame::NONE
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

        frame.show(gui.ui, |ui| {
            ui.take_available_width();
            ui.horizontal(|ui| {
                let mut state = CollapsingState::load_with_default_open(
                    ui.ctx(),
                    ui.make_persistent_id("status_panel_state"),
                    false,
                );

                let toggle_response = state.show_toggle_button(ui, paint_default_icon);
                ui.expand_to_include_rect(toggle_response.rect);

                if state.is_open() {
                    let max_height = line_height * LINE_COUNT as f32;
                    ui.set_height(max_height);

                    ScrollArea::vertical()
                        .auto_shrink(Vec2b::TRUE)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.take_available_width();
                            ui.vertical(|ui| {
                                let spacer = (max_height
                                    - status.line_count().saturating_sub(1) as f32 * line_height)
                                    .max(0.0);
                                ui.add_space(spacer);
                                ui.label(status);
                            });
                        });
                } else {
                    ui.add(Label::new(status.last_line()).truncate());
                }

                state.store(ui.ctx());
            });
        });
    }
}
