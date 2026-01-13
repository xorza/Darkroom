use egui::collapsing_header::{CollapsingState, paint_default_icon};
use egui::{Align, Frame, Label, Layout, Margin, ScrollArea, TextStyle, Vec2b};

use crate::gui::Gui;
use crate::gui::style::Style;
use common::StrExt;

const LINE_COUNT: usize = 6;

#[derive(Debug, Default)]
pub struct LogUi;

impl LogUi {
    pub fn render(&mut self, gui: &mut Gui, status: &str) {
        let style = &gui.style;

        gui.ui.horizontal(|ui| {
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

            frame.show(ui, |ui| {
                ui.take_available_width();
                ui.horizontal(|ui| {
                    let mut state = CollapsingState::load_with_default_open(
                        ui.ctx(),
                        ui.make_persistent_id("status_panel_state"),
                        false,
                    );

                    let is_open = state.is_open();

                    let toggle_response = state.show_toggle_button(ui, paint_default_icon);
                    ui.expand_to_include_rect(toggle_response.rect);

                    if is_open {
                        let line_height = ui.text_style_height(&TextStyle::Body);
                        let max_height = line_height * LINE_COUNT as f32;
                        ui.set_height(max_height);

                        ScrollArea::vertical()
                            .auto_shrink(Vec2b::new(true, true))
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                ui.vertical(|ui| {
                                    ui.take_available_width();
                                    ui.add_space(
                                        (max_height - status.line_count() as f32 * line_height)
                                            .max(0.0),
                                    );
                                    ui.label(status);
                                });
                            });
                    } else {
                        ui.add(Label::new(status.last_line().to_owned()).truncate());
                    }

                    state.store(ui.ctx());
                });
            });
            // ui.add_space(style.big_padding);
        });
    }
}
