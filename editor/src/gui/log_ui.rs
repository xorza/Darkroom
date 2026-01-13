use eframe::egui;
use egui::collapsing_header::{CollapsingState, paint_default_icon};
use egui::{Frame, Label, Margin, ScrollArea, TextStyle};

use crate::gui::Gui;
use crate::gui::style::Style;
use common::LastLine;

const EXPANDED_STATUS_LINES: usize = 5;

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
                .inner_margin(style.corner_radius * 0.5);

            frame.show(ui, |ui| {
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
                        let line_height = ui.text_style_height(&TextStyle::Body);
                        let max_height = line_height * 6.0;
                        ScrollArea::vertical()
                            .max_height(max_height)
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(status);
                            });
                    } else {
                        let mut label = Label::new(status.last_line().to_owned()).wrap();
                        label = label.truncate();
                        ui.add(label);
                    }

                    state.store(ui.ctx());
                });
            });
            // ui.add_space(style.big_padding);
        });
    }
}
