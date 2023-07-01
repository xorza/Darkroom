use std::{borrow::Cow, collections::HashMap};

use eframe::egui::{self, DragValue, TextStyle, Widget};
use egui_node_graph::*;
use uuid::Uuid;

use graph_lib::data::{DataType, Value};

pub struct MyNodeData {
    pub(crate) template: FunctionTemplate,
    pub(crate) is_active: bool,
}


#[derive(Clone)]
pub struct FunctionTemplate {
    pub(crate) function_id: Uuid,
    pub(crate) function_name: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct EditorValue(Value);

type EditorGraph = Graph<MyNodeData, DataType, EditorValue>;
type EditorState = GraphEditorState<MyNodeData, DataType, EditorValue, FunctionTemplate, MyGraphState>;

pub enum NodeCategory {}

impl CategoryTrait for NodeCategory {
    fn name(&self) -> String {
        "test_category".to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MyResponse {
    SetActiveNode(NodeId),
}

#[derive(Default)]
pub struct MyGraphState {
    pub(crate) graph: graph_lib::graph::Graph,
    pub(crate) functions: graph_lib::functions::Functions,
}

impl DataTypeTrait<MyGraphState> for DataType {
    fn data_type_color(&self, _user_state: &mut MyGraphState) -> egui::Color32 {
        match self {
            DataType::Int => egui::Color32::from_rgb(38, 109, 211),
            _ => egui::Color32::from_rgb(0, 0, 0),
        }
    }

    fn name(&self) -> Cow<'static, str> {
        self.to_string().into()
    }
}

impl NodeTemplateTrait for FunctionTemplate {
    type NodeData = MyNodeData;
    type DataType = DataType;
    type ValueType = EditorValue;
    type UserState = MyGraphState;
    type CategoryType = NodeCategory;

    fn node_finder_label(&self, user_state: &mut Self::UserState) -> Cow<'_, str> {
        let function = user_state.functions
            .function_by_node_id(self.function_id)
            .unwrap();

        function.name.clone().into()
    }

    fn node_finder_categories(&self, _user_state: &mut Self::UserState) -> Vec<Self::CategoryType> {
        vec![]
    }

    fn node_graph_label(&self, user_state: &mut Self::UserState) -> String {
        self.node_finder_label(user_state).into()
    }

    fn user_data(&self, _user_state: &mut Self::UserState) -> Self::NodeData {
        MyNodeData { template: self.clone(), is_active: false }
    }

    fn build_node(
        &self,
        graph: &mut EditorGraph,
        user_state: &mut Self::UserState,
        node_id: NodeId,
    ) {
        let input_scalar = |graph: &mut EditorGraph, name: &str| {
            graph.add_input_param(
                node_id,
                name.to_string(),
                DataType::Int,
                EditorValue(Value::Int(0)),
                InputParamKind::ConnectionOrConstant,
                true,
            );
        };

        let output_scalar = |graph: &mut EditorGraph, name: &str| {
            graph.add_output_param(node_id, name.to_string(), DataType::Int);
        };

        let function = user_state.functions.function_by_node_id(self.function_id).unwrap();
        for input in function.inputs.iter() {
            input_scalar(graph, &input.name);
        }
        for output in function.outputs.iter() {
            output_scalar(graph, &output.name);
        }
    }
}


impl WidgetValueTrait for EditorValue {
    type Response = MyResponse;
    type UserState = MyGraphState;
    type NodeData = MyNodeData;

    fn value_widget(
        &mut self,
        param_name: &str,
        _node_id: NodeId,
        ui: &mut egui::Ui,
        _user_state: &mut MyGraphState,
        _node_data: &MyNodeData,
    ) -> Vec<Self::Response> {
        match &mut self.0 {
            Value::Int(value) => {
                ui.horizontal(|ui| {
                    ui.label(param_name);
                    ui.add(DragValue::new(value));
                });
            }

            _ => {}
        }

        // This allows you to return your responses from the inline widgets.
        Vec::new()
    }
}

impl UserResponseTrait for MyResponse {}

impl NodeDataTrait for MyNodeData {
    type Response = MyResponse;
    type UserState = MyGraphState;
    type DataType = DataType;
    type ValueType = EditorValue;

    fn bottom_ui(
        &self,
        ui: &mut egui::Ui,
        node_id: NodeId,
        _graph: &EditorGraph,
        _user_state: &mut Self::UserState,
    ) -> Vec<NodeResponse<MyResponse, MyNodeData>>
        where
            MyResponse: UserResponseTrait,
    {
        let mut button: egui::Button;
        if self.is_active {
            button =
                egui::Button::new(
                    egui::RichText::new("Active")
                        .color(egui::Color32::BLACK))
                    .fill(egui::Color32::GOLD);
        } else {
            button =
                egui::Button::new(
                    egui::RichText::new("Inactive"))
                    .min_size(egui::Vec2::new(70.0, 0.0));
        }
        button = button.min_size(egui::Vec2::new(70.0, 0.0));

        let mut responses = vec![];
        if button.ui(ui).clicked() {
            responses.push(NodeResponse::User(MyResponse::SetActiveNode(node_id)));
        }

        responses
    }
}


#[derive(Default)]
pub struct NodeshopApp {
    state: EditorState,
    pub(crate) user_state: MyGraphState,
}

struct AllNodeTemplates {
    funcs: Vec<FunctionTemplate>,
}
impl AllNodeTemplates {
    fn new(funcs: &graph_lib::functions::Functions) -> Self {
        let funcs = funcs.functions().iter().map(|f|
            FunctionTemplate {
                function_name: f.name.clone(),
                function_id: f.id(),
            }
        ).collect();

        Self {
            funcs
        }
    }
}
impl NodeTemplateIter for AllNodeTemplates {
    type Item = FunctionTemplate;

    fn all_kinds(&self) -> Vec<Self::Item> {
        self.funcs.clone()
    }
}

impl eframe::App for NodeshopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::bottom("test")
            .show(ctx, |ui| {
                if ui.button("Save").clicked() {
                    println!("test");
                }
            });

        let graph_response = egui::CentralPanel::default()
            .show(ctx, |ui| {
                self.state.draw_graph_editor(
                    ui,
                    AllNodeTemplates::new(&self.user_state.functions),
                    &mut self.user_state,
                    Vec::default(),
                )
            })
            .inner;
        for node_response in graph_response.node_responses {
            match node_response {
                NodeResponse::User(user_event) => {
                    match user_event {
                        MyResponse::SetActiveNode(node) => {
                            let node = &mut self.state.graph.nodes[node].user_data;
                            node.is_active = !node.is_active;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
