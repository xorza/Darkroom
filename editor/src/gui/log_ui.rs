use eframe::egui;
use egui::collapsing_header::{CollapsingState, paint_default_icon};
use egui::{Frame, Label, Margin};

use crate::gui::style::Style;
use common::LastLine;

const EXPANDED_STATUS_LINES: usize = 5;

#[derive(Debug, Default)]
pub struct LogUi;

impl LogUi {
    pub fn render(&mut self, ctx: &egui::Context, style: &Style, status: &str) {
        assert!(
            style.corner_radius <= u8::MAX as f32,
            "style corner radius must fit inside a u8"
        );
        assert!(
            style.padding <= i8::MAX as f32,
            "style padding must fit inside an i8"
        );
        assert!(
            style.small_padding <= i8::MAX as f32,
            "style small padding must fit inside an i8"
        );

        egui::TopBottomPanel::bottom("status_panel")
            .show_separator_line(false)
            .show(ctx, |ui| {
                let mut state = CollapsingState::load_with_default_open(
                    ui.ctx(),
                    ui.make_persistent_id("status_panel_state"),
                    false,
                );

                Frame::default()
                    .fill(style.graph_background.bg_color)
                    .stroke(style.inactive_bg_stroke)
                    .corner_radius(style.corner_radius)
                    .inner_margin(Margin::from(egui::vec2(style.padding, style.small_padding)))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
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
                        });
                    });

                state.store(ui.ctx());
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
