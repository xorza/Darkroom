use std::borrow::Cow;

use eframe::egui;
use eframe::egui::{Checkbox, DragValue, TextEdit, Widget};

use egui_node_graph as eng;
use graph_lib::data::{DataType, StaticValue};
use graph_lib::function::FunctionId;
use graph_lib::graph::NodeId;

use crate::app::{AppState, MyResponse};
use crate::function_templates::FunctionTemplate;

#[derive(Clone)]
pub struct EditorNode {
    pub(crate) node_id: NodeId,
    pub(crate) function_id: FunctionId,
    pub(crate) is_output: bool,
    pub(crate) cache_outputs: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct EditorValue(pub(crate) StaticValue);

pub(crate) type EditorGraph = eng::Graph<EditorNode, DataType, EditorValue>;
pub(crate) type EditorState = eng::GraphEditorState<EditorNode, DataType, EditorValue, FunctionTemplate, AppState>;


impl eng::WidgetValueTrait for EditorValue {
    type Response = MyResponse;
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
        #[allow(clippy::single_match)]
            let mut editor_value = self.0.clone();

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
                MyResponse::SetInputValue {
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
    type Response = MyResponse;
    type UserState = AppState;
    type DataType = DataType;
    type ValueType = EditorValue;

    fn bottom_ui(
        &self,
        ui: &mut egui::Ui,
        node_id: eng::NodeId,
        _graph: &EditorGraph,
        _user_state: &mut Self::UserState,
    ) -> Vec<eng::NodeResponse<MyResponse, EditorNode>>
    where
        MyResponse: eng::UserResponseTrait,
    {
        let mut responses = vec![];

        {
            let is_output_button =
                if self.is_output {
                    egui::Button::new(
                        egui::RichText::new("Active")
                            .color(egui::Color32::BLACK)
                    )
                        .fill(egui::Color32::GOLD)
                    // .sense(egui::Sense::hover())
                } else {
                    egui::Button::new(egui::RichText::new("Active"))
                }
                    .min_size(egui::Vec2::new(70.0, 0.0));
            if is_output_button.ui(ui).clicked() {
                responses.push(eng::NodeResponse::User(MyResponse::ToggleNodeOutput(node_id)));
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
                    .min_size(egui::Vec2::new(70.0, 0.0));
            if cache_outputs_button.ui(ui).clicked() {
                responses.push(eng::NodeResponse::User(MyResponse::ToggleNodeCacheOutputs(node_id)));
            }
        }

        {
            // let function = user_state.invoker.function_by_id(self.function_id);
            // let node = user_state.graph.node_by_id(self.node_id).unwrap();
            //
            // for (index, input) in
            // function.inputs.iter().enumerate()
            //     .filter(|&(_index, input)| input.variants.is_some())
            // {
            //     let variants: &Vec<(StaticValue, String)> = input.variants
            //         .as_ref()
            //         .unwrap();
            //
            //     let mut current_value = node
            //         .inputs[index]
            //         .const_value
            //         .as_ref()
            //         .unwrap_or_else(|| {
            //             &variants
            //                 .first()
            //                 .expect("No variants")
            //                 .0
            //         })
            //         .clone();
            //
            //     let current_value_label = variants
            //         .iter()
            //         .find(|(value, _)| *value == current_value)
            //         .map(|(_, name)| name)
            //         .expect("No variant with current value");
            //
            //     egui::ComboBox::from_label(input.name.clone())
            //         .selected_text(current_value_label)
            //         .show_ui(ui, |ui| {
            //             for (value, name) in variants {
            //                 ui.selectable_value(
            //                     &mut current_value,
            //                     value.clone(),
            //                     name,
            //                 );
            //             }
            //         });
            //
            //     if Some(&current_value) != node.inputs[index].const_value.as_ref() {
            //         responses.push(eng::NodeResponse::User(
            //             MyResponse::SetInputValue {
            //                 node_id,
            //                 input_index: index as u32,
            //                 value: current_value,
            //             }
            //         ));
            //     }
            // }
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
