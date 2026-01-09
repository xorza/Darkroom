use eframe::egui;
use egui::{Align, Align2, TextEdit, UiBuilder, Vec2, pos2, vec2};
use graph::data::StaticValue;
use graph::graph::{Binding, Node, NodeId};

use crate::common::bezier::Bezier;
use crate::common::drag_value::DragValue;
use crate::gui::Gui;
use crate::gui::graph_ui::{GraphUiAction, GraphUiInteraction};
use crate::gui::node_layout::NodeLayout;
use common::BoolExt;

#[derive(Debug, Default)]
pub struct ConstBindUi {
    polyline_mesh_idx: usize,
    polyline_mesh: Vec<Bezier>,
}

impl ConstBindUi {
    pub fn start(&mut self) {
        self.polyline_mesh_idx = 0;
    }

    pub fn render(
        &mut self,
        gui: &mut Gui<'_>,
        ui_interaction: &mut GraphUiInteraction,
        node_layout: &NodeLayout,
        node: &mut Node,
    ) {
        let port_radius = gui.style.node.port_radius;
        let padding = gui.style.padding;
        let _small_padding = gui.style.small_padding;
        let mono_font = gui.style.mono_font.clone();

        for (input_idx, input) in node.inputs.iter_mut().enumerate() {
            let Binding::Const(value) = &mut input.binding else {
                continue;
            };

            let input_center = node_layout.input_center(input_idx);
            let label = const_input_text(gui, node.id, input_idx, value);

            let badge_right = input_center.x - port_radius - padding * 2.0;
            let badge_height = mono_font.size;

            let link_start = pos2(badge_right, input_center.y) + gui.style.node.const_badge_offset;
            let link_end = pos2(input_center.x - port_radius, input_center.y);

            {
                if self.polyline_mesh_idx >= self.polyline_mesh.len() {
                    self.polyline_mesh.push(Bezier::new());
                }
                let link_mesh = &mut self.polyline_mesh[self.polyline_mesh_idx];
                self.polyline_mesh_idx += 1;
                link_mesh.build_points(link_start, link_end, gui.scale);
                link_mesh.build_mesh(
                    gui.style.node.input_port_color,
                    gui.style.node.input_port_color,
                    gui.style.connections.stroke_width,
                );
                link_mesh.show(gui);
            }

            if let StaticValue::Int(value) = value {
                let drag_id = gui
                    .ui()
                    .make_persistent_id(("const_int_drag", node.id, input_idx));
                let response = {
                    DragValue::new(value, drag_id)
                        .font(mono_font.clone())
                        .color(gui.style.text_color)
                        .speed(1.0)
                        .background(
                            gui.style.inactive_bg_fill,
                            gui.style.node.const_stroke,
                            gui.style.small_corner_radius,
                        )
                        .padding(vec2(padding, 0.0))
                        .show(gui, link_start, Align2::RIGHT_CENTER)
                };
                if response.changed() {
                    ui_interaction
                        .actions
                        .push((node.id, GraphUiAction::InputChanged { input_idx }));
                }
            } else {
                let badge_rect = egui::Rect::from_min_max(
                    pos2(badge_right - 120.0, input_center.y - badge_height * 0.5),
                    pos2(badge_right, input_center.y + badge_height * 0.5),
                );
                let mut text_ui = gui.ui().new_child(UiBuilder::new().max_rect(badge_rect));
                text_ui.set_clip_rect(badge_rect);

                let text_id = gui
                    .ui()
                    .make_persistent_id(("const_input_text", node.id, input_idx));
                let mut text = label;
                let text_edit = TextEdit::singleline(&mut text)
                    .id(text_id)
                    .font(mono_font.clone())
                    .desired_width(badge_rect.width())
                    .vertical_align(Align::Center)
                    .horizontal_align(Align::Center)
                    .margin(Vec2::ZERO)
                    .clip_text(true)
                    .frame(false);
                let response = text_ui.add_sized(badge_rect.size(), text_edit);

                if response.changed()
                    && let Some(parsed) = parse_static_value(&text, value)
                    && *value != parsed
                {
                    *value = parsed;
                    ui_interaction
                        .actions
                        .push((node.id, GraphUiAction::InputChanged { input_idx }));
                }
                gui.ui().data_mut(|data| data.insert_temp(text_id, text));
            }
        }
    }
}

fn static_value_label(value: &StaticValue) -> String {
    match value {
        StaticValue::Null => "null".to_string(),
        StaticValue::Float(value) => (value.fract() < common::EPSILON as f64)
            .then_else_with(|| format!("{:.0}", value), || format!("{:.3}", value)),
        StaticValue::Int(value) => value.to_string(),
        StaticValue::Bool(value) => value.to_string(),
        StaticValue::String(value) => {
            const MAX_LEN: usize = 12;
            if value.chars().count() <= MAX_LEN {
                value.clone()
            } else {
                let truncated: String = value.chars().take(MAX_LEN).collect();
                format!("{}...", truncated)
            }
        }
    }
}

fn const_input_text(
    gui: &mut Gui<'_>,
    node_id: NodeId,
    input_idx: usize,
    value: &StaticValue,
) -> String {
    let id = gui
        .ui()
        .make_persistent_id(("const_input_text", node_id, input_idx));
    gui.ui()
        .data_mut(|data| data.get_temp::<String>(id))
        .unwrap_or_else(|| static_value_label(value))
}

fn parse_static_value(text: &str, current: &StaticValue) -> Option<StaticValue> {
    match current {
        StaticValue::Null => text
            .eq_ignore_ascii_case("null")
            .then_some(StaticValue::Null),
        StaticValue::Float(_) => text.parse::<f64>().ok().map(StaticValue::Float),
        StaticValue::Int(_) => text.parse::<i64>().ok().map(StaticValue::Int),
        StaticValue::Bool(_) => match text.trim().to_ascii_lowercase().as_str() {
            "true" => Some(StaticValue::Bool(true)),
            "false" => Some(StaticValue::Bool(false)),
            _ => None,
        },
        StaticValue::String(_) => Some(StaticValue::String(text.to_string())),
    }
}
