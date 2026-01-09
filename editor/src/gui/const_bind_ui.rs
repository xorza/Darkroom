use eframe::egui;
use egui::{
    Align, Align2, PointerButton, Pos2, Sense, Stroke, TextEdit, UiBuilder, Vec2, pos2, vec2,
};
use graph::data::StaticValue;
use graph::graph::{Binding, Node, NodeId};

use crate::common::connection_bezier::ConnectionBezier;
use crate::common::drag_value::DragValue;
use crate::gui::Gui;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_ui::{ConnectionCurve, ConnectionKey};
use crate::gui::graph_ui::{GraphUiAction, GraphUiInteraction};
use crate::gui::node_layout::NodeLayout;
use common::BoolExt;
use common::key_index_vec::{CompactInsert, KeyIndexKey, KeyIndexVec};

#[derive(Debug, Default)]
pub(crate) struct ConstBindUi {
    const_link_bezier_cache: KeyIndexVec<ConnectionKey, ConnectionCurve>,
    hovered_link: Option<ConnectionKey>,
}

impl ConstBindUi {
    pub fn start(&mut self) -> ConstBindFrame<'_> {
        ConstBindFrame::new(&mut self.const_link_bezier_cache, &mut self.hovered_link)
    }

    pub(crate) fn broke_iter(&self) -> impl Iterator<Item = &ConnectionKey> {
        self.const_link_bezier_cache
            .iter()
            .filter_map(|link| link.broke.then_some(&link.key))
    }
}

#[derive(Debug)]
pub struct ConstBindFrame<'a> {
    compact: CompactInsert<'a, ConnectionKey, ConnectionCurve>,
    hovered_link: &'a mut Option<ConnectionKey>,
    currently_hovered_link: Option<ConnectionKey>,
}

impl<'a> ConstBindFrame<'a> {
    fn new(
        polyline_mesh: &'a mut KeyIndexVec<ConnectionKey, ConnectionCurve>,
        hovered_link: &'a mut Option<ConnectionKey>,
    ) -> Self {
        Self {
            compact: polyline_mesh.compact_insert_start(),
            hovered_link,
            currently_hovered_link: None,
        }
    }

    pub fn render(
        &mut self,
        gui: &mut Gui<'_>,
        ui_interaction: &mut GraphUiInteraction,
        node_layout: &NodeLayout,
        node: &mut Node,
        breaker: Option<&ConnectionBreaker>,
    ) {
        let port_radius = gui.style.node.port_radius;
        let padding = gui.style.padding;
        let _small_padding = gui.style.small_padding;
        let mono_font = gui.style.mono_font.clone();

        for (input_idx, input) in node.inputs.iter_mut().enumerate() {
            if !matches!(input.binding, Binding::Const(_)) {
                continue;
            }

            let input_center = node_layout.input_center(input_idx);
            let badge_right = input_center.x - port_radius - padding * 2.0;
            let connection_start =
                pos2(badge_right, input_center.y) + gui.style.node.const_badge_offset;
            let connection_end = pos2(input_center.x - port_radius, input_center.y);

            let connection_key = ConnectionKey {
                input_node_id: node.id,
                input_idx,
            };
            let (_idx, curve) = self
                .compact
                .insert_with(&connection_key, || ConnectionCurve::new(connection_key));

            curve
                .bezier
                .update_points(connection_start, connection_end, gui.scale);
            curve.hovered = *self.hovered_link == Some(connection_key);
            curve.broke = curve.bezier.intersects_breaker(breaker);

            let response = curve.bezier.show(
                gui,
                Sense::click() | Sense::hover(),
                ("const_link", node.id, input_idx),
                curve.hovered,
                curve.broke,
            );

            if response.hovered() {
                self.currently_hovered_link = Some(connection_key);
            }
            if response.double_clicked_by(PointerButton::Primary) {
                input.binding = Binding::None;
                ui_interaction
                    .actions
                    .push((node.id, GraphUiAction::InputChanged { input_idx }));
                return;
            }

            let Binding::Const(value) = &mut input.binding else {
                continue;
            };

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
                            Stroke {
                                width: gui.style.node.const_stroke_width,
                                color: gui.style.node.output_port_color,
                            },
                            gui.style.small_corner_radius,
                        )
                        .padding(vec2(padding, 0.0))
                        .show(gui, connection_start, Align2::RIGHT_CENTER)
                };
                if response.changed() {
                    ui_interaction
                        .actions
                        .push((node.id, GraphUiAction::InputChanged { input_idx }));
                }
            }
        }
    }
}

impl Drop for ConstBindFrame<'_> {
    fn drop(&mut self) {
        *self.hovered_link = self.currently_hovered_link.take();
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
