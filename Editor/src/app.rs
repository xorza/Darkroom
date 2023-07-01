use std::{borrow::Cow, collections::HashMap};

use eframe::egui::{self, DragValue, TextStyle, Widget};
use egui_node_graph::*;
use uuid::Uuid;

pub struct MyNodeData {
    template: FunctionTemplate,
    is_active: bool,
}

#[derive(PartialEq, Eq)]
pub enum MyDataType {
    Scalar,
    Vec2,
}

#[derive(Copy, Clone, Debug)]
pub enum MyValueType {
    Vec2 { value: egui::Vec2 },
    Scalar { value: f32 },
}

impl Default for MyValueType {
    fn default() -> Self {
        Self::Scalar { value: 0.0 }
    }
}

impl MyValueType {
    pub fn try_to_vec2(self) -> anyhow::Result<egui::Vec2> {
        if let MyValueType::Vec2 { value } = self {
            Ok(value)
        } else {
            anyhow::bail!("Invalid cast from {:?} to vec2", self)
        }
    }

    pub fn try_to_scalar(self) -> anyhow::Result<f32> {
        if let MyValueType::Scalar { value } = self {
            Ok(value)
        } else {
            anyhow::bail!("Invalid cast from {:?} to scalar", self)
        }
    }
}

#[derive(Clone)]
pub struct FunctionTemplate {
    pub function_id: Uuid,
    pub function_name: String,
}

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

impl DataTypeTrait<MyGraphState> for MyDataType {
    fn data_type_color(&self, _user_state: &mut MyGraphState) -> egui::Color32 {
        match self {
            MyDataType::Scalar => egui::Color32::from_rgb(38, 109, 211),
            MyDataType::Vec2 => egui::Color32::from_rgb(238, 207, 109),
        }
    }

    fn name(&self) -> Cow<'static, str> {
        match self {
            MyDataType::Scalar => Cow::Borrowed("scalar"),
            MyDataType::Vec2 => Cow::Borrowed("2d vector"),
        }
    }
}

impl NodeTemplateTrait for FunctionTemplate {
    type NodeData = MyNodeData;
    type DataType = MyDataType;
    type ValueType = MyValueType;
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
        graph: &mut Graph<Self::NodeData, Self::DataType, Self::ValueType>,
        user_state: &mut Self::UserState,
        node_id: NodeId,
    ) {
        let input_scalar = |graph: &mut MyGraph, name: &str| {
            graph.add_input_param(
                node_id,
                name.to_string(),
                MyDataType::Scalar,
                MyValueType::Scalar { value: 0.0 },
                InputParamKind::ConnectionOrConstant,
                true,
            );
        };

        let output_scalar = |graph: &mut MyGraph, name: &str| {
            graph.add_output_param(node_id, name.to_string(), MyDataType::Scalar);
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

impl WidgetValueTrait for MyValueType {
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
        match self {
            MyValueType::Vec2 { value } => {
                ui.label(param_name);
                ui.horizontal(|ui| {
                    ui.label("x");
                    ui.add(DragValue::new(&mut value.x));
                    ui.label("y");
                    ui.add(DragValue::new(&mut value.y));
                });
            }
            MyValueType::Scalar { value } => {
                ui.horizontal(|ui| {
                    ui.label(param_name);
                    ui.add(DragValue::new(value));
                });
            }
        }

        // This allows you to return your responses from the inline widgets.
        Vec::new()
    }
}

impl UserResponseTrait for MyResponse {}

impl NodeDataTrait for MyNodeData {
    type Response = MyResponse;
    type UserState = MyGraphState;
    type DataType = MyDataType;
    type ValueType = MyValueType;

    fn bottom_ui(
        &self,
        ui: &mut egui::Ui,
        node_id: NodeId,
        _graph: &Graph<MyNodeData, MyDataType, MyValueType>,
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

type MyGraph = Graph<MyNodeData, MyDataType, MyValueType>;
type MyEditorState = GraphEditorState<MyNodeData, MyDataType, MyValueType, FunctionTemplate, MyGraphState>;

#[derive(Default)]
pub struct NodeshopApp {
    state: MyEditorState,
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
