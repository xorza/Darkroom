use std::default::Default;

use eframe::egui::{self};
use egui_file::{DialogType, FileDialog};

use egui_node_graph as eng;
use graph_lib::elements::basic_invoker::BasicInvoker;
use graph_lib::graph::{Binding, Graph, Node, NodeId, OutputBinding};
use graph_lib::invoke::{Invoker, UberInvoker};

use crate::eng_integration::{AppResponse, EditorState};
use crate::function_templates::FunctionTemplates;
use crate::serialization;
use crate::worker::Worker;

#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub(crate) struct ArgAddress {
    pub(crate) node_id: NodeId,
    pub(crate) arg_index: u32,
}


#[derive(Default, Debug)]
pub(crate) struct GraphState {
    pub(crate) graph: Graph,
    pub(crate) input_mapping: Vec<(eng::InputId, ArgAddress)>,
    pub(crate) output_mapping: Vec<(eng::OutputId, ArgAddress)>,
}

#[derive(Debug)]
pub struct AppState {
    invoker: UberInvoker,
    graph_state: GraphState,
    worker: Worker,
}

pub struct NodeshopApp {
    state: EditorState,
    user_state: AppState,
    function_templates: FunctionTemplates,

    file_dialog: Option<FileDialog>,
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

                    if ui.button("Run").clicked() {
                        self.user_state.worker.run_once(self.user_state.graph_state.graph.clone());
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
                        AppResponse::ToggleNodeOutput(node_id) => {
                            let eng_node = &mut self.state.graph.nodes[node_id].user_data;
                            let node = self.user_state.graph_state.graph.node_by_id_mut(eng_node.node_id).unwrap();
                            node.is_output = !node.is_output;
                            eng_node.is_output = node.is_output;
                            if node.is_output {
                                node.cache_outputs = false;
                                eng_node.cache_outputs = false;
                            }
                        }
                        AppResponse::ToggleNodeCacheOutputs(node_id) => {
                            let eng_node = &mut self.state.graph.nodes[node_id].user_data;
                            let node = self.user_state.graph_state.graph.node_by_id_mut(eng_node.node_id).unwrap();
                            node.cache_outputs = !node.cache_outputs;
                            eng_node.cache_outputs = node.cache_outputs;
                        }
                        AppResponse::SetInputValue { node_id, input_index, value } => {
                            let eng_node = &mut self.state.graph.nodes[node_id].user_data;
                            let node = self.user_state.graph_state.graph.node_by_id_mut(eng_node.node_id).unwrap();

                            eng_node.combobox_inputs
                                .iter_mut()
                                .find(|combobox_input| combobox_input.input_index == input_index)
                                .map(|combobox_input| {
                                    combobox_input.current_value = value.clone();
                                });
                            node.inputs[input_index as usize].const_value = Some(value);
                            node.inputs[input_index as usize].binding = Binding::Const;
                        }
                    }
                }

                eng::NodeResponse::ConnectEventStarted(_node_id, _parameter_id) => {}
                eng::NodeResponse::ConnectEventEnded { input: input_id, output: output_id } => {
                    let input_address = self.user_state.graph_state.input_mapping
                        .iter()
                        .find_map(|(id, address)| {
                            if *id == input_id {
                                Some(address)
                            } else {
                                None
                            }
                        })
                        .unwrap();

                    let output_address = self.user_state.graph_state.output_mapping
                        .iter()
                        .find_map(|(id, address)| {
                            if *id == output_id {
                                Some(address)
                            } else {
                                None
                            }
                        })
                        .unwrap();

                    let input_node = self.user_state.graph_state.graph
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
                    let function = self.user_state.invoker
                        .function_by_id(eng_node.user_data.function_id);
                    let node = Node::from_function(&function);
                    eng_node.user_data.node_id = node.id();

                    eng_node.inputs
                        .iter()
                        .enumerate()
                        .for_each(|(index, (_name, input_id))| {
                            self.user_state.graph_state.input_mapping.push((*input_id, ArgAddress {
                                node_id: node.id(),
                                arg_index: index as u32,
                            }));
                        });
                    eng_node.outputs
                        .iter()
                        .enumerate()
                        .for_each(|(index, (_name, output_id))| {
                            self.user_state.graph_state.output_mapping.push((*output_id, ArgAddress {
                                node_id: node.id(),
                                arg_index: index as u32,
                            }));
                        });

                    self.user_state.graph_state.graph.add_node(node);
                }
                eng::NodeResponse::SelectNode(node_id) => {
                    let eng_node = &mut self.state.graph.nodes[node_id];
                    let node = self.user_state.graph_state.graph.node_by_id_mut(eng_node.user_data.node_id).unwrap();
                    assert_eq!(node.inputs.len(), eng_node.inputs.len());
                    assert_eq!(node.outputs.len(), eng_node.outputs.len());
                }
                eng::NodeResponse::DeleteNodeUi(_node_id) => {}
                eng::NodeResponse::DeleteNodeFull { node_id: _node_id, node } => {
                    self.user_state.graph_state.graph.remove_node_by_id(node.user_data.node_id);
                }
                eng::NodeResponse::DisconnectEvent { input: input_id, output: _output_id } => {
                    let input_address = self.user_state.graph_state.input_mapping
                        .iter()
                        .find_map(|(id, address)| {
                            if *id == input_id {
                                Some(address)
                            } else {
                                None
                            }
                        })
                        .unwrap();

                    let input_node = self.user_state.graph_state.graph
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
                            DialogType::OpenFile => self.load_yaml(filename).unwrap_or_default(),
                            DialogType::SaveFile => self.save_yaml(filename).unwrap_or_default(),

                            _ => panic!("Invalid dialog type")
                        }
                    }
                }
                self.file_dialog = None;
            }
        }
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
            user_state: AppState {
                invoker,
                graph_state: GraphState::default(),
                worker: Worker::new(),
            },
            file_dialog: None,
        }
    }
}

impl NodeshopApp {
    fn save_yaml(&self, filename: &str) -> anyhow::Result<()> {
        serialization::save(
            &self.user_state.graph_state,
            &self.state,
            filename,
        )
    }
    fn load_yaml(&mut self, filename: &str) -> anyhow::Result<()> {
        let (
            graph_state,
            editor_state
        ) = serialization::load(&self.user_state.invoker, filename)?;

        self.user_state.graph_state = graph_state;
        self.state = editor_state;

        Ok(())
    }
}