use std::borrow::Cow;

use eframe::egui;
use eframe::egui::{Checkbox, DragValue, TextEdit, Widget};

use egui_node_graph as eng;
use graph_lib::data::{DataType, StaticValue};
use graph_lib::function::FunctionId;
use graph_lib::graph::NodeId;

use crate::app::AppState;
use crate::function_templates::FunctionTemplate;

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
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct EditorValue(pub(crate) StaticValue);

pub(crate) type EditorGraph = eng::Graph<EditorNode, DataType, EditorValue>;
pub(crate) type EditorState = eng::GraphEditorState<EditorNode, DataType, EditorValue, FunctionTemplate, AppState>;

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

            _ => {}
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
    type DataType = DataType;
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

impl eng::DataTypeTrait<AppState> for DataType {
    fn data_type_color(&self, _user_state: &mut AppState) -> egui::Color32 {
        match self {
            DataType::Int
            | DataType::Float => egui::Color32::from_rgb(38, 109, 211),
            DataType::Bool => egui::Color32::from_rgb(211, 38, 109),
            DataType::String => egui::Color32::from_rgb(109, 211, 38),
            _ => egui::Color32::from_rgb(0, 0, 0),
        }
    }

    fn name(&self) -> Cow<'static, str> {
        self.to_string().into()
    }
}
