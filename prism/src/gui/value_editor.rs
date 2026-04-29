use std::sync::Arc;

use egui::{Align2, Pos2, Response, vec2};
use scenarium::data::{DataType, EnumDef, FsPathMode, StaticValue};

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::style::DragValueStyle;
use crate::gui::widgets::drag_value::DragValueNumeric;
use crate::gui::widgets::{ComboBox, DragValue, FilePicker, FilePickerMode};

fn picker_mode(mode: FsPathMode) -> FilePickerMode {
    match mode {
        FsPathMode::ExistingFile => FilePickerMode::ExistingFile,
        FsPathMode::NewFile => FilePickerMode::NewFile,
        FsPathMode::Directory => FilePickerMode::Directory,
    }
}

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
    anchor: Align2,
    style: Option<DragValueStyle>,
}

impl<'a> StaticValueEditor<'a> {
    pub fn new(value: &'a mut StaticValue, data_type: &'a DataType) -> Self {
        Self {
            value,
            data_type,
            pos: Pos2::ZERO,
            anchor: Align2::RIGHT_CENTER,
            style: None,
        }
    }

    pub fn pos(mut self, pos: Pos2) -> Self {
        assert!(pos.x.is_finite() && pos.y.is_finite());
        self.pos = pos;
        self
    }

    pub fn style(mut self, style: DragValueStyle) -> Self {
        self.style = Some(style);
        self
    }

    pub fn show(self, gui: &mut Gui<'_>, id: StableId) -> Response {
        let style = self
            .style
            .unwrap_or_else(|| gui.style.node.const_bind_style.clone());

        match self.value {
            StaticValue::Int(int_value) => {
                render_numeric_drag(gui, id, int_value, 1.0, self.pos, self.anchor, style)
            }

            StaticValue::Float(float_value) => {
                render_numeric_drag(gui, id, float_value, 0.01, self.pos, self.anchor, style)
            }

            StaticValue::Enum {
                type_id,
                variant_name,
            } => {
                let DataType::Enum(enum_def) = self.data_type else {
                    panic!("Expected Enum data type for StaticValue::Enum");
                };
                assert_eq!(*type_id, enum_def.type_id, "Type ID mismatch");

                render_enum_dropdown(gui, id, enum_def, variant_name, self.pos, &style)
            }

            StaticValue::FsPath { config, path } => {
                FilePicker::new(id, path, &config.extensions, picker_mode(config.mode))
                    .pos(self.pos)
                    .anchor(self.anchor)
                    .style(style)
                    .show(gui)
            }

            StaticValue::Null => todo!("StaticValueEditor: Null variant not implemented"),
            StaticValue::Bool(_) => todo!("StaticValueEditor: Bool variant not implemented"),
            StaticValue::String(_) => todo!("StaticValueEditor: String variant not implemented"),
        }
    }
}

/// Shared numeric (Int / Float) editor — both `StaticValue::Int` and
/// `StaticValue::Float` produce the same widget, differing only in the
/// drag `speed` and the underlying value type.
#[allow(clippy::too_many_arguments)]
fn render_numeric_drag<T: DragValueNumeric>(
    gui: &mut Gui<'_>,
    id: StableId,
    value: &mut T,
    speed: f32,
    pos: Pos2,
    anchor: Align2,
    style: DragValueStyle,
) -> Response {
    DragValue::new(id, value)
        .font(gui.style.mono_font.clone())
        .color(gui.style.text_color)
        .speed(speed)
        .padding(vec2(gui.style.small_padding, 0.0))
        .pos(pos)
        .anchor(anchor)
        .style(style)
        .show(gui)
}

fn render_enum_dropdown(
    gui: &mut Gui<'_>,
    id: StableId,
    enum_def: &Arc<EnumDef>,
    variant_name: &mut String,
    pos: Pos2,
    style: &DragValueStyle,
) -> Response {
    ComboBox::new(id, variant_name, &enum_def.variants)
        .font(gui.style.sub_font.clone())
        .color(gui.style.text_color)
        .padding(vec2(gui.style.small_padding, 0.0))
        .pos(pos)
        .anchor(Align2::RIGHT_CENTER)
        .style(style.clone())
        .show(gui)
}
