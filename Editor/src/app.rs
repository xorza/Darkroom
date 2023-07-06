use std::{borrow::Cow, collections::HashMap};

use eframe::egui::{self, DragValue, Widget};
use egui_file::{DialogType, FileDialog};
use serde::{Deserialize, Serialize};

use egui_node_graph as eng;
use graph_lib::data::{DataType, Value};
use graph_lib::functions::{Function, FunctionId};
use graph_lib::graph::{Binding, Input, NodeId, Output};

#[derive(Clone, Debug, Default)]
pub struct EditorNode {
    template: FunctionTemplate,
    is_output: bool,
}

#[derive(Clone, Debug, Default)]
struct FunctionTemplate(Function);

#[derive(Clone, Debug, PartialEq, Default)]
pub struct EditorValue(Value);

enum NodeCategory {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MyResponse {
    ToggleNodeOutput(eng::NodeId),
}

type EditorGraph = eng::Graph<EditorNode, DataType, EditorValue>;
type EditorState = eng::GraphEditorState<EditorNode, DataType, EditorValue, FunctionTemplate, MyState>;

#[derive(Default, Clone, Debug)]
struct FunctionTemplates {
    templates: Vec<FunctionTemplate>,
}

#[derive(Default)]
pub struct MyState {
    function_templates: FunctionTemplates,
}

#[derive(Default)]
pub struct NodeshopApp {
    state: EditorState,
    user_state: MyState,
    function_templates: FunctionTemplates,

    file_dialog: Option<FileDialog>,
}


impl eng::CategoryTrait for NodeCategory {
    fn name(&self) -> String {
        "test_category".to_string()
    }
}

impl eng::DataTypeTrait<MyState> for DataType {
    fn data_type_color(&self, _user_state: &mut MyState) -> egui::Color32 {
        match self {
            DataType::Int => egui::Color32::from_rgb(38, 109, 211),
            _ => egui::Color32::from_rgb(0, 0, 0),
        }
    }

    fn name(&self) -> Cow<'static, str> {
        self.to_string().into()
    }
}

impl eng::NodeTemplateTrait for FunctionTemplate {
    type NodeData = EditorNode;
    type DataType = DataType;
    type ValueType = EditorValue;
    type UserState = MyState;
    type CategoryType = NodeCategory;

    fn node_finder_label(&self, user_state: &mut Self::UserState) -> Cow<'_, str> {
        let function = user_state.function_templates
            .function_by_id(self.0.id())
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
        EditorNode { template: self.clone(), is_output: self.0.is_output }
    }

    fn build_node(
        &self,
        graph: &mut EditorGraph,
        user_state: &mut Self::UserState,
        node_id: eng::NodeId,
    ) {
        let input_scalar = |graph: &mut EditorGraph, name: &str| {
            graph.add_input_param(
                node_id,
                name.to_string(),
                DataType::Int,
                EditorValue(Value::Int(0)),
                eng::InputParamKind::ConnectionOrConstant,
                true,
            );
        };

        let output_scalar = |graph: &mut EditorGraph, name: &str| {
            graph.add_output_param(node_id, name.to_string(), DataType::Int);
        };

        let function = user_state.function_templates.function_by_id(self.0.id()).unwrap();
        for input in function.inputs.iter() {
            input_scalar(graph, &input.name);
        }
        for output in function.outputs.iter() {
            output_scalar(graph, &output.name);
        }
    }
}

impl eng::WidgetValueTrait for EditorValue {
    type Response = MyResponse;
    type UserState = MyState;
    type NodeData = EditorNode;

    fn value_widget(
        &mut self,
        param_name: &str,
        _node_id: eng::NodeId,
        ui: &mut egui::Ui,
        _user_state: &mut MyState,
        _node_data: &EditorNode,
    ) -> Vec<Self::Response> {
        #[allow(clippy::single_match)]
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

impl eng::UserResponseTrait for MyResponse {}

impl eng::NodeDataTrait for EditorNode {
    type Response = MyResponse;
    type UserState = MyState;
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
        let mut button: egui::Button;
        if self.is_output {
            button =
                egui::Button::new(
                    egui::RichText::new("Output")
                        .color(egui::Color32::BLACK))
                    .fill(egui::Color32::GOLD);
        } else {
            button = egui::Button::new(egui::RichText::new("Output"));
        }
        button = button.min_size(egui::Vec2::new(70.0, 0.0));

        if button.ui(ui).clicked() {
            vec![eng::NodeResponse::User(MyResponse::ToggleNodeOutput(node_id))]
        } else {
            vec![]
        }
    }
}

impl FunctionTemplates {
    fn load(&mut self, funcs: &graph_lib::functions::Functions) {
        let funcs = funcs
            .functions()
            .iter()
            .map(|f| FunctionTemplate(f.clone()))
            .collect();

        self.templates = funcs;
    }

    fn function_by_id(&self, id: FunctionId) -> Option<&graph_lib::functions::Function> {
        self.templates.iter().find(|f| f.0.id() == id).map(|f| &f.0)
    }
}

impl eng::NodeTemplateIter for FunctionTemplates {
    type Item = FunctionTemplate;

    fn all_kinds(&self) -> &Vec<Self::Item> {
        &self.templates
    }
}

impl eframe::App for NodeshopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::bottom("test")
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Open").clicked() {
                        let mut dialog = FileDialog::open_file(None);
                        dialog.open();
                        self.file_dialog = Some(dialog);
                    }

                    if ui.button("Save").clicked() {
                        let mut dialog = FileDialog::save_file(None);
                        dialog.open();
                        self.file_dialog = Some(dialog);
                    }
                });
            });

        let graph_response = egui::CentralPanel::default()
            .show(ctx, |ui| {
                self.state.draw_graph_editor(
                    ui,
                    &self.function_templates,
                    &mut self.user_state,
                    Vec::default(),
                )
            })
            .inner;
        for node_response in graph_response.node_responses {
            #[allow(clippy::single_match)]
            match node_response {
                eng::NodeResponse::User(user_event) => {
                    match user_event {
                        MyResponse::ToggleNodeOutput(node) => {
                            let node = &mut self.state.graph.nodes[node].user_data;
                            node.is_output = !node.is_output;
                        }
                    }
                }
                _ => {}
            }
        }

        if let Some(dialog) = &mut self.file_dialog {
            if dialog.show(ctx).selected() {
                if let Some(file) = dialog.path() {
                    if let Some(filename) = file.to_str() {
                        match dialog.dialog_type() {
                            DialogType::OpenFile => self.load_graph_from_yaml(filename).unwrap_or_default(),
                            DialogType::SaveFile => self.save_graph_to_yaml(filename).unwrap_or_default(),

                            _ => panic!("Invalid dialog type")
                        }
                    }
                }
                self.file_dialog = None;
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq, Hash, Clone)]
struct ArgAddress {
    node_id: NodeId,
    arg_index: usize,
}

#[derive(Default, Serialize, Deserialize)]
struct SerializedGraph {
    graph: graph_lib::graph::Graph,
    positions: HashMap<NodeId, (f32, f32)>,
}

impl NodeshopApp {
    pub fn load_functions_from_yaml_file(&mut self, path: &str) -> anyhow::Result<()> {
        let mut funcs = graph_lib::functions::Functions::default();
        funcs.load_yaml_file(path)?;
        self.function_templates.load(&funcs);
        self.user_state.function_templates = self.function_templates.clone();

        Ok(())
    }

    fn save_graph_to_yaml(&self, filename: &str) -> anyhow::Result<()> {
        let editor_graph = &self.state.graph;

        let mut graph = SerializedGraph::default();

        let mut input_addresses = HashMap::<eng::InputId, ArgAddress>::new();
        let mut output_addresses = HashMap::<eng::OutputId, ArgAddress>::new();

        for (editor_node_id, editor_node) in editor_graph.nodes.iter() {
            let mut node = graph_lib::graph::Node::new();
            node.name = editor_node.user_data.template.0.name.clone();
            node.function_id = editor_node.user_data.template.0.id();
            node.is_output = editor_node.user_data.template.0.is_output;
            node.behavior = editor_node.user_data.template.0.behavior;

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
                        binding: Binding::None,
                        const_value: Some(editor_value.clone()),
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

            let position = self.state.node_positions.get(editor_node_id).unwrap();
            graph.positions.insert(node.id(), (position.x, position.y));

            graph.graph.add_node(node);
        }

        for (editor_input_id, editor_output_id) in editor_graph.connections.iter() {
            let input_address = input_addresses.get(&editor_input_id).unwrap();
            let output_address = output_addresses.get(editor_output_id).unwrap();

            let input = graph.graph
                .node_by_id_mut(input_address.node_id)
                .unwrap()
                .inputs
                .get_mut(input_address.arg_index)
                .unwrap();

            input.binding = Binding::from_output_binding(
                output_address.node_id,
                output_address.arg_index as u32,
            );
        }

        let yaml = serde_yaml::to_string(&graph)?;
        std::fs::write(filename, yaml)?;

        Ok(())
    }

    fn load_graph_from_yaml(&mut self, filename: &str) -> anyhow::Result<()> {
        let yaml = std::fs::read_to_string(filename)?;
        let graph: SerializedGraph = serde_yaml::from_str(&yaml)?;

        self.state = EditorState::default();
        let editor_graph = &mut self.state.graph;


        let mut input_addresses = HashMap::<ArgAddress, eng::InputId>::new();
        let mut output_addresses = HashMap::<ArgAddress, eng::OutputId>::new();

        for (_index, node) in graph.graph.nodes().iter().enumerate() {
            let function = self.user_state.function_templates
                .function_by_id(node.function_id)
                .unwrap();

            let node_data = EditorNode {
                template: FunctionTemplate(function.clone()),
                is_output: node.is_output,
            };

            let node_add =
                |editor_graph: &mut EditorGraph, editor_node_id: eng::NodeId| {
                    for (index, input) in node.inputs.iter().enumerate() {
                        let default_value = input.const_value.clone()
                            .unwrap_or(Value::from(input.data_type));

                        let input_id = editor_graph.add_input_param(
                            editor_node_id,
                            input.name.clone(),
                            input.data_type,
                            EditorValue(default_value),
                            eng::InputParamKind::ConnectionOrConstant,
                            true);

                        input_addresses.insert(
                            ArgAddress {
                                node_id: node.id(),
                                arg_index: index,
                            },
                            input_id);
                    }

                    for (index, output) in node.outputs.iter().enumerate() {
                        let output_id = editor_graph.add_output_param(
                            editor_node_id,
                            output.name.clone(),
                            output.data_type);

                        output_addresses.insert(
                            ArgAddress {
                                node_id: node.id(),
                                arg_index: index,
                            },
                            output_id);
                    }
                };

            let node_id = editor_graph.add_node(node.name.clone(), node_data, node_add);
            self.state.node_order.push(node_id);

            if let Some((x, y)) = graph.positions
                .get(&node.id()) { self.state.node_positions.insert(
                        node_id,
                        egui::Pos2 { x: *x, y: *y },
                    ); }
        }

        for node in graph.graph.nodes().iter() {
            for (index, input) in node.inputs.iter().enumerate() {
                if let Some(binding) = input.binding.as_output_binding() {
                    let input_id = input_addresses.get(
                        &ArgAddress {
                            node_id: node.id(),
                            arg_index: index,
                        }).unwrap();

                    let output_id = output_addresses.get(
                        &ArgAddress {
                            node_id: binding.output_node_id,
                            arg_index: binding.output_index as usize,
                        }).unwrap();

                    editor_graph.add_connection(*output_id, *input_id);
                }
            }
        }

        Ok(())
    }
}
