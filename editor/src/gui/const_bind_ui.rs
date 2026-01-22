use std::sync::Arc;

use egui::{Align2, PointerButton, Pos2, Response, Sense, pos2, vec2};
use graph::data::{DataType, EnumDef, StaticValue};
use graph::graph::{Binding, Node, NodeId};
use graph::prelude::Func;

use crate::common::combo_box::ComboBox;
use crate::common::connection_bezier::ConnectionBezierStyle;
use crate::common::drag_value::DragValue;
use crate::common::file_picker::FilePicker;
use crate::gui::Gui;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_ui::{ConnectionCurve, ConnectionKey, PortKind};
use crate::gui::graph_ui_interaction::GraphUiInteraction;
use crate::gui::node_layout::NodeLayout;
use crate::gui::style::DragValueStyle;
use crate::model::graph_ui_action::GraphUiAction;
use common::key_index_vec::{CompactInsert, KeyIndexVec};

// === ConstBindUi ===

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

// === ConstBindFrame ===

#[derive(Debug)]
pub struct ConstBindFrame<'a> {
    compact: CompactInsert<'a, ConnectionKey, ConnectionCurve>,
    prev_hovered_connection: &'a mut Option<ConnectionKey>,
    currently_hovered_connection: Option<ConnectionKey>,
}

impl<'a> ConstBindFrame<'a> {
    fn new(
        cache: &'a mut KeyIndexVec<ConnectionKey, ConnectionCurve>,
        hovered_connection: &'a mut Option<ConnectionKey>,
    ) -> Self {
        Self {
            compact: cache.compact_insert_start(),
            prev_hovered_connection: hovered_connection,
            currently_hovered_connection: None,
        }
    }

    pub fn render(
        &mut self,
        gui: &mut Gui<'_>,
        ui_interaction: &mut GraphUiInteraction,
        node_layout: &NodeLayout,
        node: &mut Node,
        func: &Func,
        breaker: Option<&ConnectionBreaker>,
    ) {
        for (input_idx, input) in node.inputs.iter_mut().enumerate() {
            let Binding::Const(value) = &mut input.binding else {
                continue;
            };

            let ctx = InputContext {
                node_id: node.id,
                input_idx,
                func,
                breaker,
            };

            if self.render_const_input(gui, ui_interaction, node_layout, &ctx, value) {
                input.binding = Binding::None;
                return;
            }
        }
    }

    fn render_const_input(
        &mut self,
        gui: &mut Gui<'_>,
        ui_interaction: &mut GraphUiInteraction,
        node_layout: &NodeLayout,
        ctx: &InputContext<'_>,
        value: &mut StaticValue,
    ) -> bool {
        let connection_key = ConnectionKey::Input {
            input_node_id: ctx.node_id,
            input_idx: ctx.input_idx,
        };
        let prev_hovered = *self.prev_hovered_connection == Some(connection_key);

        let (connection_start, connection_end) =
            compute_connection_points(gui, node_layout, ctx.input_idx);

        let (_idx, curve) = self
            .compact
            .insert_with(&connection_key, || ConnectionCurve::new(connection_key));

        curve
            .bezier
            .update_points_with_count(connection_start, connection_end, gui.scale(), 3);

        let prev_broke = ctx.breaker.is_some() && curve.broke;
        let style =
            ConnectionBezierStyle::build(&gui.style, PortKind::Input, prev_broke, prev_hovered);

        let bezier_response = curve.bezier.show(
            gui,
            Sense::click() | Sense::hover(),
            ("const_link", ctx.node_id, ctx.input_idx),
            style,
        );

        let mut currently_hovered = bezier_response.hovered();
        let mut currently_broke = curve.bezier.intersects_breaker(ctx.breaker);

        // Double-click to clear binding
        if bezier_response.double_clicked_by(PointerButton::Primary) {
            ui_interaction.add_action(GraphUiAction::InputChanged {
                node_id: ctx.node_id,
                input_idx: ctx.input_idx,
                before: Binding::Const(value.clone()),
                after: Binding::None,
            });
            return true;
        }

        let const_bind_style = build_const_bind_style(
            gui,
            prev_broke || currently_broke,
            prev_hovered || currently_hovered,
        );

        let before_value = value.clone();
        let editor_response =
            render_value_editor(gui, ctx, value, connection_start, &const_bind_style);

        if let Some(breaker) = ctx.breaker {
            currently_broke |= breaker.intersects_rect(editor_response.rect);
        }
        currently_hovered |= editor_response.hovered();

        if before_value != *value {
            currently_hovered = true;
            ui_interaction.add_action(GraphUiAction::InputChanged {
                node_id: ctx.node_id,
                input_idx: ctx.input_idx,
                before: Binding::Const(before_value),
                after: Binding::Const(value.clone()),
            });
        }

        if currently_hovered {
            self.currently_hovered_connection = Some(connection_key);
        }
        curve.hovered = currently_hovered;
        curve.broke = currently_broke;

        false
    }
}

impl Drop for ConstBindFrame<'_> {
    fn drop(&mut self) {
        *self.prev_hovered_connection = self.currently_hovered_connection.take();
    }
}

// === Helpers ===

struct InputContext<'a> {
    node_id: NodeId,
    input_idx: usize,
    func: &'a Func,
    breaker: Option<&'a ConnectionBreaker>,
}

fn compute_connection_points(
    gui: &Gui<'_>,
    node_layout: &NodeLayout,
    input_idx: usize,
) -> (Pos2, Pos2) {
    let input_center = node_layout.input_center(input_idx);
    let port_radius = gui.style.node.port_radius;
    let padding = gui.style.padding;

    let badge_right = input_center.x - port_radius - padding * 2.0;
    let start = pos2(badge_right, input_center.y) + gui.style.node.const_badge_offset;
    let end = pos2(input_center.x - port_radius, input_center.y);

    (start, end)
}

fn build_const_bind_style(gui: &Gui<'_>, is_broke: bool, is_hovered: bool) -> DragValueStyle {
    let stroke_color = if is_broke {
        gui.style.connections.broke_clr
    } else if is_hovered {
        gui.style.node.output_hover_color
    } else {
        gui.style.node.output_port_color
    };

    let mut style = gui.style.node.const_bind_style.clone();
    style.stroke.color = stroke_color;
    style
}

fn render_value_editor(
    gui: &mut Gui<'_>,
    ctx: &InputContext<'_>,
    value: &mut StaticValue,
    pos: Pos2,
    style: &DragValueStyle,
) -> Response {
    let small_padding = gui.style.small_padding;
    let mono_font = gui.style.mono_font.clone();
    let text_color = gui.style.text_color;

    match value {
        StaticValue::Int(int_value) => DragValue::new(int_value)
            .font(mono_font)
            .color(text_color)
            .speed(1.0)
            .padding(vec2(small_padding, 0.0))
            .pos(pos)
            .align(Align2::RIGHT_CENTER)
            .style(style.clone())
            .show(gui, ("const_int_drag", ctx.node_id, ctx.input_idx)),

        StaticValue::Float(float_value) => DragValue::new(float_value)
            .font(mono_font)
            .color(text_color)
            .speed(0.01)
            .padding(vec2(small_padding, 0.0))
            .pos(pos)
            .align(Align2::RIGHT_CENTER)
            .style(style.clone())
            .show(gui, ("const_float_drag", ctx.node_id, ctx.input_idx)),

        StaticValue::Enum {
            type_id,
            variant_name,
        } => {
            let DataType::Enum(enum_def) = &ctx.func.inputs[ctx.input_idx].data_type else {
                panic!("Expected Enum data type");
            };
            assert_eq!(*type_id, enum_def.type_id, "Type ID mismatch");

            render_enum_dropdown(
                gui,
                ("const_enum_dropdown", ctx.node_id, ctx.input_idx),
                enum_def,
                variant_name,
                pos,
                style,
            )
        }

        StaticValue::FsPath(path) => {
            let DataType::FsPath(config) = &ctx.func.inputs[ctx.input_idx].data_type else {
                panic!("Expected FsPath data type");
            };
            FilePicker::new(path, config)
                .pos(pos)
                .align(Align2::RIGHT_CENTER)
                .style(style.clone())
                .show(gui, ("const_fspath_input", ctx.node_id, ctx.input_idx))
        }

        _ => todo!(),
    }
}

fn render_enum_dropdown(
    gui: &mut Gui<'_>,
    id_salt: impl std::hash::Hash,
    enum_def: &Arc<EnumDef>,
    variant_name: &mut String,
    pos: Pos2,
    style: &DragValueStyle,
) -> Response {
    ComboBox::new(variant_name, &enum_def.variants)
        .font(gui.style.sub_font.clone())
        .color(gui.style.text_color)
        .padding(vec2(gui.style.small_padding, 0.0))
        .pos(pos)
        .align(Align2::RIGHT_CENTER)
        .style(style.clone())
        .show(gui, id_salt)
}
