use eframe::egui;
use egui::{Align, DragValue, Stroke, StrokeKind, TextEdit, UiBuilder, Vec2, pos2};
use graph::data::StaticValue;
use graph::graph::{Binding, Node, NodeId};

use crate::common::font::ScaledFontId;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_ui::{GraphUiAction, GraphUiInteraction};
use crate::gui::node_layout::NodeLayout;
use common::BoolExt;

pub fn render_const_bindings(
    ctx: &mut GraphContext,
    ui_interaction: &mut GraphUiInteraction,
    node_layout: &NodeLayout,
    node: &mut Node,
) {
    let port_radius = ctx.style.node.port_radius * ctx.scale;
    let padding = ctx.style.small_padding * ctx.scale;
    let font = ctx.style.sub_font.scaled(ctx.scale);

    for (input_idx, input) in node.inputs.iter_mut().enumerate() {
        let Binding::Const(value) = &mut input.binding else {
            continue;
        };

        let input_center = node_layout.input_center(input_idx);
        let label = const_input_text(ctx, node.id, input_idx, value);
        let label_galley = ctx
            .painter
            .layout_no_wrap(label, font.clone(), ctx.style.text_color);
        let badge_width = label_galley.size().x + padding * 2.0;
        let badge_right = input_center.x - port_radius - padding * 2.0;
        let badge_height = label_galley.size().y;
        let badge_rect = egui::Rect::from_min_max(
            pos2(
                badge_right - badge_width,
                input_center.y - badge_height * 0.5,
            ),
            pos2(badge_right, input_center.y + badge_height * 0.5),
        );

        let link_start = pos2(badge_rect.max.x, input_center.y);
        let link_end = pos2(input_center.x - port_radius, input_center.y);

        ctx.painter.line_segment(
            [link_start, link_end],
            Stroke::new(
                ctx.style.connections.stroke_width,
                ctx.style.node.input_port_color,
            ),
        );
        ctx.painter.rect(
            badge_rect,
            ctx.style.small_corner_radius * ctx.scale,
            ctx.style.inactive_bg_fill,
            ctx.style.node.const_stroke,
            StrokeKind::Outside,
        );

        let mut text_ui = ctx.ui.new_child(UiBuilder::new().max_rect(badge_rect));
        text_ui.set_clip_rect(badge_rect);

        if let StaticValue::Int(value) = value {
            let mut new_value = *value;
            let drag = DragValue::new(&mut new_value)
                .speed(1.0)
                .custom_formatter(|value, _| format!("{value:.0}"));
            let prev_style = text_ui.style().clone();
            let mut style = (*prev_style).clone();
            style.override_font_id = Some(font.clone());
            let mut visuals = style.visuals.clone();
            visuals.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
            visuals.widgets.inactive.bg_stroke = Stroke::NONE;
            visuals.widgets.hovered.bg_fill = egui::Color32::TRANSPARENT;
            visuals.widgets.hovered.bg_stroke = Stroke::NONE;
            visuals.widgets.active.bg_fill = egui::Color32::TRANSPARENT;
            visuals.widgets.active.bg_stroke = Stroke::NONE;
            visuals.widgets.open.bg_fill = egui::Color32::TRANSPARENT;
            visuals.widgets.open.bg_stroke = Stroke::NONE;
            style.visuals = visuals;
            text_ui.set_style(style);
            let response = text_ui.add_sized(badge_rect.size(), drag);
            text_ui.set_style(prev_style);
            if response.changed() && new_value != *value {
                *value = new_value;
                ui_interaction
                    .actions
                    .push((node.id, GraphUiAction::InputChanged { input_idx }));
            }
        } else {
            let text_id = ctx
                .ui
                .make_persistent_id(("const_input_text", node.id, input_idx));
            let mut text = const_input_text(ctx, node.id, input_idx, value);
            let text_edit = TextEdit::singleline(&mut text)
                .id(text_id)
                .font(font.clone())
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
            ctx.ui.data_mut(|data| data.insert_temp(text_id, text));
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
    ctx: &GraphContext,
    node_id: NodeId,
    input_idx: usize,
    value: &StaticValue,
) -> String {
    let id = ctx
        .ui
        .make_persistent_id(("const_input_text", node_id, input_idx));
    ctx.ui
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
