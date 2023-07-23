use std::borrow::Cow;

use eframe::egui;
use eframe::egui::{Checkbox, DragValue, TextEdit, Widget};

use egui_node_graph as eng;
use egui_node_graph::{InputId, OutputId};
use graph_lib::data::{DataType, StaticValue, TypeId};
use graph_lib::function::{Function, FunctionId};
use graph_lib::graph::NodeId;

use crate::app::AppState;
use crate::function_templates::FunctionTemplate;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum EditorDataType {
    Int,
    Float,
    Bool,
    String,
    Event,
    Custom {
        type_id: TypeId,
        // type_name is not included in the hash or equality check
        type_name: String,
    },
}

#[derive(Debug)]
pub(crate) struct ComboboxInput {
    pub(crate) input_index: u32,
    pub(crate) name: String,
    pub(crate) current_value: StaticValue,
    pub(crate) variants: Vec<(StaticValue, String)>,
}

#[derive(Debug)]
pub struct EditorNode {
    pub(crate) node_id: NodeId,
    pub(crate) function_id: FunctionId,
    pub(crate) is_output: bool,
    pub(crate) cache_outputs: bool,

    pub(crate) combobox_inputs: Vec<ComboboxInput>,

    pub(crate) trigger_id: InputId,
    pub(crate) inputs: Vec<InputId>,
    pub(crate) events: Vec<OutputId>,
    pub(crate) outputs: Vec<OutputId>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct EditorValue(pub(crate) StaticValue);

pub(crate) type EditorGraph = eng::Graph<EditorNode, EditorDataType, EditorValue>;
pub(crate) type EditorState = eng::GraphEditorState<EditorNode, EditorDataType, EditorValue, FunctionTemplate, AppState>;

#[derive(Clone, Debug)]
pub enum AppResponse {
    ToggleNodeOutput(eng::NodeId),
    ToggleNodeCacheOutputs(eng::NodeId),
    SetInputValue {
        node_id: eng::NodeId,
        input_index: u32,
        value: StaticValue,
    },

}


impl eng::UserResponseTrait for AppResponse {}

impl eng::WidgetValueTrait for EditorValue {
    type Response = AppResponse;
    type UserState = AppState;
    type NodeData = EditorNode;

    fn value_widget(
        &mut self,
        param_index: usize,
        param_name: &str,
        node_id: eng::NodeId,
        ui: &mut egui::Ui,
        _user_state: &mut AppState,
        _node_data: &EditorNode,
    ) -> Vec<Self::Response> {
        let mut editor_value = self.0.clone();

        #[allow(clippy::single_match)]
        match &mut editor_value {
            StaticValue::Int(value) => {
                ui.horizontal(|ui| {
                    ui.label(param_name);
                    ui.add(DragValue::new(value));
                });
            }
            StaticValue::Float(value) => {
                ui.horizontal(|ui| {
                    ui.label(param_name);
                    ui.add(DragValue::new(value));
                });
            }
            StaticValue::String(value) => {
                ui.horizontal(|ui| {
                    ui.label(param_name);
                    ui.add(TextEdit::singleline(value));
                });
            }
            StaticValue::Bool(value) => {
                ui.horizontal(|ui| {
                    ui.label(param_name);
                    ui.add(Checkbox::new(value, ""));
                });
            }

            _ => {
                ui.label(param_name);
            }
        }

        if editor_value != self.0 {
            self.0 = editor_value.clone();

            vec![
                AppResponse::SetInputValue {
                    node_id,
                    input_index: param_index as u32,
                    value: editor_value,
                }
            ]
        } else {
            vec![]
        }
    }
}

impl eng::NodeDataTrait for EditorNode {
    type Response = AppResponse;
    type UserState = AppState;
    type DataType = EditorDataType;
    type ValueType = EditorValue;

    fn bottom_ui(
        &self,
        ui: &mut egui::Ui,
        node_id: eng::NodeId,
        _graph: &EditorGraph,
        _user_state: &mut Self::UserState,
    ) -> Vec<eng::NodeResponse<AppResponse, EditorNode>>
    where
        AppResponse: eng::UserResponseTrait,
    {
        let mut responses = vec![];
        ui.set_width(130.0);

        ui.horizontal(|ui| {
            {
                let is_output_button =
                    if self.is_output {
                        egui::Button::new(
                            egui::RichText::new("Active")
                                .color(egui::Color32::BLACK)
                        )
                            .fill(egui::Color32::GOLD)
                    } else {
                        egui::Button::new(egui::RichText::new("Active"))
                    }
                        .min_size(egui::Vec2::new(50.0, 0.0));
                if is_output_button.ui(ui).clicked() {
                    responses.push(eng::NodeResponse::User(AppResponse::ToggleNodeOutput(node_id)));
                }
            }

            if !self.is_output {
                let cache_outputs_button =
                    if self.cache_outputs {
                        egui::Button::new(
                            egui::RichText::new("Once")
                                .color(egui::Color32::BLACK)
                        )
                            .fill(egui::Color32::GOLD)
                    } else {
                        egui::Button::new(egui::RichText::new("Once"))
                    }
                        .min_size(egui::Vec2::new(50.0, 0.0));
                if cache_outputs_button.ui(ui).clicked() {
                    responses.push(eng::NodeResponse::User(AppResponse::ToggleNodeCacheOutputs(node_id)));
                }
            }
        });

        {
            for combo_input in self.combobox_inputs.iter() {
                let variants: &Vec<(StaticValue, String)> = &combo_input.variants;
                let mut current_value = combo_input.current_value.clone();

                let current_value_label = variants
                    .iter()
                    .find(|(value, _)| *value == current_value)
                    .map(|(_, name)| name)
                    .expect("No variant with current value");

                egui::ComboBox::from_label(combo_input.name.clone())
                    .selected_text(current_value_label)
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        for (value, name) in variants {
                            ui.selectable_value(
                                &mut current_value,
                                value.clone(),
                                name,
                            );
                        }
                    })
                ;

                if current_value != combo_input.current_value {
                    responses.push(eng::NodeResponse::User(
                        AppResponse::SetInputValue {
                            node_id,
                            input_index: combo_input.input_index,
                            value: current_value,
                        }
                    ));
                }
            }
        }

        responses
    }
}

impl eng::DataTypeTrait<AppState> for EditorDataType {
    fn data_type_color(&self, _user_state: &mut AppState) -> egui::Color32 {
        match self {
            EditorDataType::Int
            | EditorDataType::Float => egui::Color32::from_rgb(38, 109, 211),
            EditorDataType::Bool => egui::Color32::from_rgb(211, 38, 109),
            EditorDataType::String => egui::Color32::from_rgb(109, 211, 38),
            EditorDataType::Event => egui::Color32::from_rgb(255, 109, 109),
            _ => egui::Color32::from_rgb(255, 255, 255),
        }
    }

    fn name(&self) -> Cow<'static, str> {
        self.to_string().into()
    }
}

impl EditorDataType {}

impl From<DataType> for EditorDataType {
    fn from(value: DataType) -> Self {
        EditorDataType::from(&value)
    }
}

impl From<&DataType> for EditorDataType {
    fn from(value: &DataType) -> Self {
        match value {
            DataType::Int => EditorDataType::Int,
            DataType::Float => EditorDataType::Float,
            DataType::Bool => EditorDataType::Bool,
            DataType::String => EditorDataType::String,
            DataType::Custom { type_id, type_name } => EditorDataType::Custom {
                type_id: *type_id,
                type_name: type_name.clone(),
            },

            _ => unimplemented!(),
        }
    }
}

impl ToString for EditorDataType {
    fn to_string(&self) -> String {
        match self {
            EditorDataType::Int => "Int".to_string(),
            EditorDataType::Float => "Float".to_string(),
            EditorDataType::Bool => "Bool".to_string(),
            EditorDataType::String => "String".to_string(),
            EditorDataType::Event => "Event".to_string(),
            EditorDataType::Custom { type_name, .. } => type_name.clone(),
        }
    }
}


pub(crate) fn build_node_from_func(
    editor_graph: &mut EditorGraph,
    function: &Function,
    eng_node_id: eng::NodeId,
) {
    let trigger_id = editor_graph.add_input_param(
        eng_node_id,
        "trigger".to_string(),
        EditorDataType::Event,
        EditorValue::default(),
        eng::InputParamKind::ConnectionOnly,
        true,
    );

    let inputs = function.inputs
        .iter()
        .filter(|input| input.variants.is_none())
        .map(|input| {
            let shown_inline = input.variants.is_none();
            let param_kind = if input.data_type.is_custom() {
                eng::InputParamKind::ConnectionOnly
            } else {
                eng::InputParamKind::ConnectionOrConstant
            };

            let value = input.default_value
                .as_ref()
                .map(|value| {
                    value.clone()
                })
                .unwrap_or_else(|| {
                    StaticValue::from(&input.data_type)
                });

            let input_id = editor_graph.add_input_param(
                eng_node_id,
                input.name.to_string(),
                (&input.data_type).into(),
                EditorValue(value),
                param_kind,
                shown_inline,
            );

            input_id
        })
        .collect::<Vec<InputId>>();

    let events = function.events
        .iter()
        .map(|event| {
            let event_id = editor_graph.add_output_param(
                eng_node_id,
                event.clone(),
                EditorDataType::Event,
            );

            event_id
        })
        .collect::<Vec<OutputId>>();

    let outputs = function.outputs
        .iter()
        .map(|output| {
            let output_id = editor_graph.add_output_param(
                eng_node_id,
                output.name.to_string(),
                (&output.data_type).into(),
            );

            output_id
        })
        .collect::<Vec<OutputId>>();


    let editor_node = &mut editor_graph.nodes[eng_node_id].user_data;
    editor_node.trigger_id = trigger_id;
    editor_node.inputs = inputs;
    editor_node.events = events;
    editor_node.outputs = outputs;
}

pub(crate) fn combobox_inputs_from_function(function: &Function) -> Vec<ComboboxInput> {
    let combobox_inputs = function.inputs
        .iter()
        .enumerate()
        .filter_map(|(index, input)| {
            if let Some(variants) = input.variants.as_ref() {
                let variants = variants
                    .iter()
                    .map(|variant| (variant.0.clone(), variant.1.clone()))
                    .collect::<Vec<(StaticValue, String)>>();
                let current_value = input.default_value.as_ref()
                    .unwrap_or_else(|| {
                        &variants
                            .first()
                            .expect("No variants")
                            .0
                    })
                    .clone();

                Some(ComboboxInput {
                    input_index: index as u32,
                    name: input.name.clone(),
                    current_value,
                    variants,
                })
            } else {
                None
            }
        })
        .collect::<Vec<ComboboxInput>>();

    combobox_inputs
}

