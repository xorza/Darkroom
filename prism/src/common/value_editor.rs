use std::sync::Arc;

use egui::{Align2, Pos2, Response, vec2};
use scenarium::data::{DataType, EnumDef, StaticValue};

use crate::common::combo_box::ComboBox;
use crate::common::drag_value::DragValue;
use crate::common::file_picker::FilePicker;
use crate::gui::Gui;
use crate::gui::style::DragValueStyle;

/// Editor for `StaticValue` types with consistent styling.
///
/// Handles rendering of different value types:
/// - Int: Draggable integer field
/// - Float: Draggable float field
/// - Enum: Dropdown selector
/// - FsPath: File picker with browse button
#[derive(Debug)]
pub struct StaticValueEditor<'a> {
    value: &'a mut StaticValue,
    data_type: &'a DataType,
    pos: Pos2,
    align: Align2,
    style: Option<DragValueStyle>,
}

impl<'a> StaticValueEditor<'a> {
    pub fn new(value: &'a mut StaticValue, data_type: &'a DataType) -> Self {
        Self {
            value,
            data_type,
            pos: Pos2::ZERO,
            align: Align2::RIGHT_CENTER,
            style: None,
        }
    }

    pub fn pos(mut self, pos: Pos2) -> Self {
        assert!(pos.x.is_finite() && pos.y.is_finite());
        self.pos = pos;
        self
    }

    pub fn align(mut self, align: Align2) -> Self {
        self.align = align;
        self
    }

    pub fn style(mut self, style: DragValueStyle) -> Self {
        self.style = Some(style);
        self
    }

    pub fn show(self, gui: &mut Gui<'_>, id_salt: impl std::hash::Hash) -> Response {
        let small_padding = gui.style.small_padding;
        let mono_font = gui.style.mono_font.clone();
        let text_color = gui.style.text_color;
        let style = self
            .style
            .unwrap_or_else(|| gui.style.node.const_bind_style.clone());

        match self.value {
            StaticValue::Int(int_value) => DragValue::new(int_value)
                .font(mono_font)
                .color(text_color)
                .speed(1.0)
                .padding(vec2(small_padding, 0.0))
                .pos(self.pos)
                .align(self.align)
                .style(style)
                .show(gui, id_salt),

            StaticValue::Float(float_value) => DragValue::new(float_value)
                .font(mono_font)
                .color(text_color)
                .speed(0.01)
                .padding(vec2(small_padding, 0.0))
                .pos(self.pos)
                .align(self.align)
                .style(style)
                .show(gui, id_salt),

            StaticValue::Enum {
                type_id,
                variant_name,
            } => {
                let DataType::Enum(enum_def) = self.data_type else {
                    panic!("Expected Enum data type for StaticValue::Enum");
                };
                assert_eq!(*type_id, enum_def.type_id, "Type ID mismatch");

                render_enum_dropdown(gui, id_salt, enum_def, variant_name, self.pos, &style)
            }

            StaticValue::FsPath(path) => {
                let DataType::FsPath(config) = self.data_type else {
                    panic!("Expected FsPath data type for StaticValue::FsPath");
                };
                FilePicker::new(path, config)
                    .pos(self.pos)
                    .align(self.align)
                    .style(style)
                    .show(gui, id_salt)
            }

            _ => {
                // For unsupported types, return a dummy response
                // This allows gradual addition of new value types
                gui.ui()
                    .allocate_response(egui::Vec2::ZERO, egui::Sense::hover())
            }
        }
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
