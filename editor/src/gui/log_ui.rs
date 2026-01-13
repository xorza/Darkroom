use eframe::egui;
use egui::collapsing_header::{CollapsingState, paint_default_icon};
use egui::{Frame, Label, Margin};

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

                    let display_text = if state.is_open() {
                        status_tail(status, EXPANDED_STATUS_LINES)
                    } else {
                        status.last_line().to_owned()
                    };

                    let mut label = Label::new(display_text).wrap();
                    if !state.is_open() {
                        label = label.truncate();
                    }
                    ui.add(label);

                    state.store(ui.ctx());
                });
            });
            // ui.add_space(style.big_padding);
        });
    }
}

fn status_tail(status: &str, lines_to_show: usize) -> String {
    assert!(
        lines_to_show > 0,
        "status lines to show must be greater than zero"
    );

    let lines: Vec<&str> = status.lines().collect();
    let total = lines.len();
    let start = total.saturating_sub(lines_to_show);
    lines[start..].join("\n")
}
