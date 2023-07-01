use std::{borrow::Cow, collections::HashMap};

use eframe::egui::{self, DragValue, TextStyle, Widget};
use egui_node_graph::*;
use uuid::Uuid;

use graph_lib::data::{DataType, Value};
use graph_lib::graph::{Binding, FunctionBehavior, Input, Output};

#[derive(Clone, Debug, Default)]
pub struct EditorNode {
    template: FunctionTemplate,
    behavior: FunctionBehavior,
}

#[derive(Clone, Debug, Default)]
struct FunctionTemplate {
    function_id: Uuid,
    name: String,
    is_output: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct EditorValue(Value);

enum NodeCategory {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MyResponse {
    SetActiveNode(NodeId),
}

#[derive(Default)]
pub struct MyGraphState {
    functions: graph_lib::functions::Functions,
}

type EditorGraph = Graph<EditorNode, DataType, EditorValue>;
type EditorState = GraphEditorState<EditorNode, DataType, EditorValue, FunctionTemplate, MyGraphState>;

#[derive(Default)]
pub struct NodeshopApp {
    state: EditorState,
    user_state: MyGraphState,
}
struct AllNodeTemplates {
    funcs: Vec<FunctionTemplate>,
}


impl CategoryTrait for NodeCategory {
    fn name(&self) -> String {
        "test_category".to_string()
    }
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
    type NodeData = EditorNode;
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
        EditorNode { template: self.clone(), behavior: FunctionBehavior::Passive }
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
    type NodeData = EditorNode;

    fn value_widget(
        &mut self,
        param_name: &str,
        _node_id: NodeId,
        ui: &mut egui::Ui,
        _user_state: &mut MyGraphState,
        _node_data: &EditorNode,
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

impl NodeDataTrait for EditorNode {
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
    ) -> Vec<NodeResponse<MyResponse, EditorNode>>
        where
            MyResponse: UserResponseTrait,
    {
        let mut button: egui::Button;
        if self.behavior == FunctionBehavior::Active {
            button =
                egui::Button::new(
                    egui::RichText::new("Active")
                        .color(egui::Color32::BLACK))
                    .fill(egui::Color32::GOLD);
        } else {
            button = egui::Button::new(egui::RichText::new("Passive"));
        }
        button = button.min_size(egui::Vec2::new(70.0, 0.0));

        let mut responses = vec![];
        if button.ui(ui).clicked() {
            responses.push(NodeResponse::User(MyResponse::SetActiveNode(node_id)));
        }

        responses
    }
}

impl AllNodeTemplates {
    fn new(funcs: &graph_lib::functions::Functions) -> Self {
        let funcs = funcs.functions().iter().map(|f|
            FunctionTemplate {
                name: f.name.clone(),
                function_id: f.id(),
                is_output: f.is_output,
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
                    self.save_graph_to_yaml();
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
                            node.behavior.toggle();
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

impl NodeshopApp {
    pub fn load_functions_from_yaml_file(&mut self, path: &str) {
        self.user_state.functions
            .load_yaml_file(path)
            .expect("Failed to load test_functions.yml");
    }

    fn save_graph_to_yaml(&self) {
        let graph = self.create_graph();
        let yaml = graph.to_yaml().expect("Failed to save graph to yaml");
        std::fs::write("graph.yml", yaml).expect("Failed to write test_graph.yml");
    }

    fn create_graph(&self) -> graph_lib::graph::Graph {
        let editor_graph = &self.state.graph;

        let mut graph = graph_lib::graph::Graph::default();

        struct ArgAddress {
            node_id: Uuid,
            arg_index: usize,
        }
        let mut input_addresses = HashMap::<InputId, ArgAddress>::new();
        let mut output_addresses = HashMap::<OutputId, ArgAddress>::new();

        for (_editor_node_id, editor_node) in editor_graph.nodes.iter() {
            let mut node = graph_lib::graph::Node::new();
            node.name = editor_node.user_data.template.name.clone();
            node.function_id = editor_node.user_data.template.function_id;
            node.is_output = editor_node.user_data.template.is_output;
            node.behavior = editor_node.user_data.behavior;

            editor_node.inputs.iter()
                .for_each(|(editor_input_name, editor_input_id)| {
                    let editor_input = editor_graph.inputs
                        .get(*editor_input_id).unwrap();
                    let editor_value = &editor_input.value.0;

                    assert_eq!(editor_input.typ, editor_value.data_type());

                    node.inputs.push(Input {
                        name: editor_input_name.clone(),
                        data_type: editor_input.typ,
                        is_required: true,
                        binding: None,
                        default_value: Some(editor_value.clone()),
                    });

                    input_addresses.insert(*editor_input_id, ArgAddress {
                        node_id: node.id(),
                        arg_index: node.inputs.len() - 1,
                    });
                });

            editor_node.outputs.iter()
                .for_each(|(editor_output_name, editor_output_id)| {
                    let editor_output = editor_graph.outputs.get(*editor_output_id).unwrap();

                    node.outputs.push(Output {
                        name: editor_output_name.clone(),
                        data_type: editor_output.typ,
                    });

                    output_addresses.insert(*editor_output_id, ArgAddress {
                        node_id: node.id(),
                        arg_index: node.outputs.len() - 1,
                    });
                });

            graph.add_node(node);
        }

        for (editor_input_id, editor_output_id) in editor_graph.connections.iter() {
            let input_address = input_addresses.get(&editor_input_id).unwrap();
            let output_address = output_addresses.get(&editor_output_id).unwrap();

            let input = graph
                .node_by_id_mut(input_address.node_id)
                .unwrap()
                .inputs
                .get_mut(input_address.arg_index)
                .unwrap();

            input.binding = Some(Binding::new(
                output_address.node_id,
                output_address.arg_index as u32,
            ));
        }

        graph
    }
}
