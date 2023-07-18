use std::borrow::Cow;
use std::default::Default;

use eframe::egui::{self, Checkbox, DragValue, TextEdit, Widget};
use egui_file::{DialogType, FileDialog};
use serde::{Deserialize, Serialize};

use egui_node_graph as eng;
use graph_lib::data::{DataType, StaticValue};
use graph_lib::elements::basic_invoker::BasicInvoker;
use graph_lib::function::{Function, FunctionId};
use graph_lib::graph::{Binding, Graph, Node, NodeId, OutputBinding};
use graph_lib::invoke::{Invoker, UberInvoker};

type Positions = Vec<(NodeId, (f32, f32))>;

#[derive(Debug, Eq, PartialEq, Hash, Clone)]
struct ArgAddress {
    node_id: NodeId,
    arg_index: u32,
}

#[derive(Clone)]
pub struct EditorNode {
    node_id: NodeId,
    function_id: FunctionId,
    is_output: bool,
    cache_outputs: bool,
}

#[derive(Clone, Debug, Default)]
struct FunctionTemplate(Function);

#[derive(Clone, Debug, PartialEq, Default)]
pub struct EditorValue(StaticValue);

enum NodeCategory {}

#[derive(Clone, Debug)]
pub enum MyResponse {
    ToggleNodeOutput(eng::NodeId),
    ToggleNodeCacheOutputs(eng::NodeId),
    SetInputValue {
        node_id: eng::NodeId,
        input_index: u32,
        value: StaticValue,
    },

}

type EditorGraph = eng::Graph<EditorNode, DataType, EditorValue>;
type EditorState = eng::GraphEditorState<EditorNode, DataType, EditorValue, FunctionTemplate, MyState>;

#[derive(Default, Clone, Debug)]
struct FunctionTemplates {
    templates: Vec<FunctionTemplate>,
}

#[derive(Default)]
pub struct MyState {
    invoker: UberInvoker,
    graph: Graph,
    input_mapping: Vec<(eng::InputId, ArgAddress)>,
    output_mapping: Vec<(eng::OutputId, ArgAddress)>,
}

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

fn build_node_from_func(
    editor_graph: &mut EditorGraph,
    function: &Function,
    eng_node_id: eng::NodeId,
) {
    function.inputs
        .iter()
        .filter(|input| input.variants.is_none())
        .for_each(|input| {
            let shown_inline = input.variants.is_none();
            let param_kind = if input.data_type.is_custom() {
                eng::InputParamKind::ConnectionOnly
            } else {
                eng::InputParamKind::ConnectionOrConstant
            };

            let _input_id = editor_graph.add_input_param(
                eng_node_id,
                input.name.to_string(),
                input.data_type.clone(),
                EditorValue(StaticValue::from(&input.data_type)),
                param_kind,
                shown_inline,
            );
        });

    function.outputs
        .iter()
        .for_each(|output| {
            let _output_id = editor_graph.add_output_param(
                eng_node_id,
                output.name.to_string(),
                output.data_type.clone(),
            );
        });
}

impl eng::NodeTemplateTrait for FunctionTemplate {
    type NodeData = EditorNode;
    type DataType = DataType;
    type ValueType = EditorValue;
    type UserState = MyState;
    type CategoryType = NodeCategory;

    fn node_finder_label(&self, _user_state: &mut Self::UserState) -> Cow<'_, str> {
        self.0.name.as_str().into()
    }

    fn node_finder_categories(&self, _user_state: &mut Self::UserState) -> Vec<Self::CategoryType> {
        vec![]
    }

    fn node_graph_label(&self, user_state: &mut Self::UserState) -> String {
        self.node_finder_label(user_state).into()
    }

    fn user_data(&self, _user_state: &mut Self::UserState) -> Self::NodeData {
        EditorNode {
            function_id: self.0.self_id,
            node_id: NodeId::nil(),
            is_output: self.0.is_output,
            cache_outputs: false,
        }
    }

    fn build_node(
        &self,
        graph: &mut EditorGraph,
        _user_state: &mut Self::UserState,
        node_id: eng::NodeId,
    ) {
        let function = &self.0;
        build_node_from_func(
            graph,
            function,
            node_id,
        );
    }
}

impl eng::WidgetValueTrait for EditorValue {
    type Response = MyResponse;
    type UserState = MyState;
    type NodeData = EditorNode;

    fn value_widget(
        &mut self,
        param_index: usize,
        param_name: &str,
        node_id: eng::NodeId,
        ui: &mut egui::Ui,
        _user_state: &mut MyState,
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
        user_state: &mut Self::UserState,
    ) -> Vec<eng::NodeResponse<MyResponse, EditorNode>>
    where
        MyResponse: eng::UserResponseTrait,
    {
        let mut responses = vec![];
        let function = user_state.invoker.function_by_id(self.function_id);
        let node = user_state.graph.node_by_id(self.node_id).unwrap();

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
            for (index, input) in
            function.inputs.iter().enumerate()
                .filter(|&(_index, input)| input.variants.is_some())
            {
                let variants: &Vec<(StaticValue, String)> = input.variants
                    .as_ref()
                    .unwrap();

                let mut current_value = node
                    .inputs[index]
                    .const_value
                    .as_ref()
                    .unwrap_or_else(|| {
                        &variants
                            .first()
                            .expect("No variants")
                            .0
                    })
                    .clone();

                let current_value_label = variants
                    .iter()
                    .find(|(value, _)| *value == current_value)
                    .map(|(_, name)| name)
                    .expect("No variant with current value");

                egui::ComboBox::from_label(input.name.clone())
                    .selected_text(current_value_label)
                    .show_ui(ui, |ui| {
                        for (value, name) in variants {
                            ui.selectable_value(
                                &mut current_value,
                                value.clone(),
                                name,
                            );
                        }
                    });

                if Some(&current_value) != node.inputs[index].const_value.as_ref() {
                    responses.push(eng::NodeResponse::User(
                        MyResponse::SetInputValue {
                            node_id,
                            input_index: index as u32,
                            value: current_value,
                        }
                    ));
                }
            }
        }

        responses
    }
}

impl FunctionTemplates {
    fn function_by_id(&self, id: FunctionId) -> Option<&Function> {
        self.templates.iter().find(|f| f.0.self_id == id).map(|f| &f.0)
    }
}

impl From<Vec<Function>> for FunctionTemplates {
    fn from(functions: Vec<Function>) -> Self {
        let templates = functions.iter()
            .map(|f| FunctionTemplate(f.clone()))
            .collect();

        Self { templates }
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
                        MyResponse::ToggleNodeOutput(node_id) => {
                            let eng_node = &mut self.state.graph.nodes[node_id].user_data;
                            let node = self.user_state.graph.node_by_id_mut(eng_node.node_id).unwrap();
                            node.is_output = !node.is_output;
                            eng_node.is_output = node.is_output;
                            if node.is_output {
                                node.cache_outputs = false;
                                eng_node.cache_outputs = false;
                            }
                        }
                        MyResponse::ToggleNodeCacheOutputs(node_id) => {
                            let eng_node = &mut self.state.graph.nodes[node_id].user_data;
                            let node = self.user_state.graph.node_by_id_mut(eng_node.node_id).unwrap();
                            node.cache_outputs = !node.cache_outputs;
                            eng_node.cache_outputs = node.cache_outputs;
                        }
                        MyResponse::SetInputValue { node_id, input_index, value } => {
                            let eng_node = &mut self.state.graph.nodes[node_id].user_data;
                            let node = self.user_state.graph.node_by_id_mut(eng_node.node_id).unwrap();
                            node.inputs[input_index as usize].const_value = Some(value);
                        }
                    }
                }

                eng::NodeResponse::ConnectEventStarted(_node_id, _parameter_id) => {}
                eng::NodeResponse::ConnectEventEnded { input: input_id, output: output_id } => {
                    let input_address = self.user_state.input_mapping
                        .iter()
                        .find_map(|(id, address)| {
                            if *id == input_id {
                                Some(address)
                            } else {
                                None
                            }
                        })
                        .unwrap();

                    let output_address = self.user_state.output_mapping
                        .iter()
                        .find_map(|(id, address)| {
                            if *id == output_id {
                                Some(address)
                            } else {
                                None
                            }
                        })
                        .unwrap();

                    let input_node = self.user_state.graph
                        .node_by_id_mut(input_address.node_id)
                        .unwrap();

                    input_node.inputs[input_address.arg_index as usize].binding =
                        Binding::Output(OutputBinding {
                            output_node_id: output_address.node_id,
                            output_index: output_address.arg_index as u32,
                        });
                }
                eng::NodeResponse::CreatedNode(node_id) => {
                    let eng_node = &mut self.state.graph.nodes[node_id];
                    let function = self.user_state.invoker.function_by_id(eng_node.user_data.function_id);
                    let node = Node::from_function(&function);
                    eng_node.user_data.node_id = node.id();

                    eng_node.inputs
                        .iter()
                        .enumerate()
                        .for_each(|(index, (_name, input_id))| {
                            self.user_state.input_mapping.push((*input_id, ArgAddress {
                                node_id: node.id(),
                                arg_index: index as u32,
                            }));
                        });
                    eng_node.outputs
                        .iter()
                        .enumerate()
                        .for_each(|(index, (_name, output_id))| {
                            self.user_state.output_mapping.push((*output_id, ArgAddress {
                                node_id: node.id(),
                                arg_index: index as u32,
                            }));
                        });

                    self.user_state.graph.add_node(node);
                }
                eng::NodeResponse::SelectNode(node_id) => {
                    let eng_node = &mut self.state.graph.nodes[node_id];
                    let node = self.user_state.graph.node_by_id_mut(eng_node.user_data.node_id).unwrap();
                    assert_eq!(node.inputs.len(), eng_node.inputs.len());
                    assert_eq!(node.outputs.len(), eng_node.outputs.len());
                }
                eng::NodeResponse::DeleteNodeUi(_node_id) => {}
                eng::NodeResponse::DeleteNodeFull { node_id: _node_id, node } => {
                    self.user_state.graph.remove_node_by_id(node.user_data.node_id);
                }
                eng::NodeResponse::DisconnectEvent { input: input_id, output: _output_id } => {
                    let input_address = self.user_state.input_mapping
                        .iter()
                        .find_map(|(id, address)| {
                            if *id == input_id {
                                Some(address)
                            } else {
                                None
                            }
                        })
                        .unwrap();

                    let input_node = self.user_state.graph
                        .node_by_id_mut(input_address.node_id)
                        .unwrap();

                    let input = &mut input_node.inputs[input_address.arg_index as usize];
                    if input.const_value.is_some() {
                        input.binding = Binding::Const;
                    } else {
                        input.binding = Binding::None;
                    }
                }
                eng::NodeResponse::RaiseNode(_node_id) => {}
                eng::NodeResponse::MoveNode { node: _node_id, drag_delta: _delta } => {}
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

impl NodeshopApp {
    fn save_graph_to_yaml(&self, filename: &str) -> anyhow::Result<()> {
        let filename = common::get_file_extension(filename)
            .ok()
            .and_then(|ext| {
                if ext == "yaml" {
                    Some(filename.to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| format!("{}.yaml", filename));

        let positions: Positions = self.state.graph.nodes
            .iter()
            .map(|(editor_node_id, editor_node)| {
                let position = self.state.node_positions.get(editor_node_id).unwrap();
                (editor_node.user_data.node_id, (position.x, position.y))
            })
            .collect();

        let graph = &self.user_state.graph;

        let file = std::fs::File::create(filename)?;
        let mut writer = serde_yaml::Serializer::new(file);
        graph.serialize(&mut writer)?;
        positions.serialize(&mut writer)?;

        Ok(())
    }

    fn load_graph_from_yaml(&mut self, filename: &str) -> anyhow::Result<()> {
        let file = std::fs::File::open(filename)?;
        let mut deserializer = serde_yaml::Deserializer::from_reader(file);

        self.state = EditorState::default();
        self.user_state.graph = Graph::deserialize(deserializer.next().unwrap())?;
        let positions = Positions::deserialize(deserializer.next().unwrap())?;

        drop(deserializer);

        for node in self.user_state.graph.nodes() {
            let function = self.user_state.invoker.function_by_id(node.function_id);
            let editor_node = EditorNode {
                node_id: node.id(),
                function_id: node.function_id,
                is_output: node.is_output,
                cache_outputs: node.cache_outputs,
            };

            let eng_node_id = self.state.graph.add_node(
                node.name.clone(),
                editor_node,
                |_, _| {},
            );
            self.state.node_order.push(eng_node_id);

            build_node_from_func(
                &mut self.state.graph,
                &function,
                eng_node_id,
            );

            let eng_node = &self.state.graph.nodes[eng_node_id];
            eng_node.inputs
                .iter()
                .enumerate()
                .for_each(|(index, (_name, input_id))| {
                    self.user_state.input_mapping.push((
                        *input_id,
                        ArgAddress {
                            node_id: node.id(),
                            arg_index: index as u32,
                        },
                    ));
                });
            eng_node.outputs
                .iter()
                .enumerate()
                .for_each(|(index, (_name, output_id))| {
                    self.user_state.output_mapping.push((
                        *output_id,
                        ArgAddress {
                            node_id: node.id(),
                            arg_index: index as u32,
                        },
                    ));
                });

            // set default values
            node.inputs
                .iter()
                .zip(eng_node.inputs.iter())
                .for_each(|(input, (_name, eng_input_id))| {
                    if let Some(const_value) = &input.const_value {
                        self.state.graph.inputs[*eng_input_id].value.0 = const_value.clone();
                    }
                });

            positions
                .iter()
                .find(|(id, _)| *id == node.id())
                .map(|(_, (x, y))| {
                    self.state.node_positions.insert(eng_node_id, egui::Pos2 { x: *x, y: *y });
                });
        }

        // Connect inputs to outputs
        self.user_state.graph.nodes()
            .iter()
            .flat_map(|node|
                node.inputs
                    .iter()
                    .enumerate()
                    .map(|(index, input)|
                        (node.id(), index, &input.binding)
                    )
            )
            .filter(|(_node_id, _index, binding)|
                binding.is_output_binding()
            )
            .for_each(|(node_id, index, binding)| {
                let input_address = ArgAddress {
                    node_id,
                    arg_index: index as u32,
                };
                let output_address = binding
                    .as_output_binding()
                    .map(|output_binding| {
                        ArgAddress {
                            node_id: output_binding.output_node_id,
                            arg_index: output_binding.output_index,
                        }
                    }).unwrap();

                let input_id = self.user_state.input_mapping
                    .iter()
                    .find(|(_, address)| *address == input_address)
                    .map(|(input_id, _)| *input_id)
                    .unwrap();
                let output_id = self.user_state.output_mapping
                    .iter()
                    .find(|(_, address)| *address == output_address)
                    .map(|(output_id, _)| *output_id)
                    .unwrap();

                self.state.graph.add_connection(output_id, input_id);
            });

        Ok(())
    }
}

impl Default for NodeshopApp {
    fn default() -> Self {
        let invoker = UberInvoker::new(
            vec![
                Box::new(BasicInvoker::default()),
            ]
        );
        let function_templates = FunctionTemplates::from(
            invoker.all_functions()
        );

        NodeshopApp {
            state: EditorState::default(),
            function_templates,
            user_state: MyState {
                invoker,
                ..Default::default()
            },
            file_dialog: None,
        }
    }
}

