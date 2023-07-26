use std::default::Default;
use std::sync::{Arc, Mutex};

use eframe::CreationContext;
use eframe::egui::{self};
use egui_file::{DialogType, FileDialog};

use common::apply::ApplyMut;
use egui_node_graph as eng;
use graph_lib::elements::basic_invoker::{BasicInvoker, Logger};
use graph_lib::elements::timers_invoker::TimersInvoker;
use graph_lib::function::Function;
use graph_lib::graph::{Binding, Graph, Node, OutputBinding};
use graph_lib::invoke_context::Invoker;
use graph_lib::worker::Worker;

use crate::arg_mapping::{ArgMapping, FindByInputIdResult, FindByOutputIdResult};
use crate::eng_integration::{AppResponse, EditorNode, EditorState, register_node};
use crate::function_templates::FunctionTemplates;
use crate::serialization;

#[derive(Default, Debug)]
pub(crate) struct GraphState {
    pub(crate) graph: Graph,
    pub(crate) arg_mapping: ArgMapping,
}

#[derive(Debug)]
pub struct AppState {
    functions: Vec<Function>,
    graph_state: GraphState,
    worker: Worker,
    egui_ctx: egui::Context,
    logger: Logger,
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
                ui.vertical(|ui| {
                    ui.vertical(|ui| {
                        ui.set_height(110.0);
                        ui.label("Log:");
                        ui.vertical(|ui| {
                            let mut logger = self.user_state.logger.lock().unwrap();
                            if logger.len() > 5 {
                                let remaining = logger.len() - 5;
                                logger.drain(..remaining);
                            }

                            for log in logger.iter() {
                                ui.label(log);
                            }
                        });
                    });

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

        graph_response.node_responses
            .into_iter()
            .for_each(|node_response| self.response(node_response));

        self.file_dialog(ctx);
    }
}

impl NodeshopApp {
    pub(crate) fn new(cc: &CreationContext) -> Self {
        let egui_ctx = cc.egui_ctx.clone();
        let functions: Arc<Mutex<Vec<Function>>> = Arc::new(Mutex::new(Vec::new()));

        let functions_clone = functions.clone();
        let worker = Worker::new(
            move |logger| {
                load_invokers(logger, functions_clone.lock().unwrap().as_mut())
            },
            move || egui_ctx.request_repaint(),
        );

        let functions = functions.lock().unwrap().clone();
        let function_templates = FunctionTemplates::from(
            functions.clone()
        );
        let logger = worker.logger.clone();

        NodeshopApp {
            state: EditorState::default(),
            function_templates,
            user_state: AppState {
                functions,
                graph_state: GraphState::default(),
                worker,
                egui_ctx: cc.egui_ctx.clone(),
                logger,
            },
            file_dialog: None,
        }
    }

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
        ) = serialization::load(&self.user_state.functions, filename)?;

        self.user_state.graph_state = graph_state;
        self.state = editor_state;

        Ok(())
    }

    fn file_dialog(&mut self, ctx: &egui::Context) {
        let dialog =
            self.file_dialog
                .as_mut()
                .and_then(|dialog| {
                    if dialog.show(ctx).selected() {
                        Some(dialog)
                    } else {
                        None
                    }
                })
                .and_then(|dialog| {
                    dialog.path()
                        .and_then(|path| {
                            path
                                .to_str()
                                .map(|path| path.to_string())
                        })
                        .map(|path|
                            (path, dialog.dialog_type())
                        )
                });

        if let Some((filename, dialog_type)) = dialog {
            self.file_dialog = None;

            match dialog_type {
                DialogType::OpenFile =>
                    self
                        .load_yaml(filename.as_str())
                        .unwrap_or_default(),
                DialogType::SaveFile =>
                    self
                        .save_yaml(filename.as_str())
                        .unwrap_or_default(),

                _ => panic!("Invalid dialog type")
            }
        }
    }
    fn user_response(&mut self, user_event: AppResponse) {
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
                    .apply_mut(|combobox_input| {
                        combobox_input.current_value = value.clone();
                    });
                node.inputs[input_index as usize].const_value = Some(value);
                node.inputs[input_index as usize].binding = Binding::Const;
            }
        }
    }
    fn response(&mut self, node_response: eng::NodeResponse<AppResponse, EditorNode>) {
        #[allow(clippy::single_match)]
        match node_response {
            eng::NodeResponse::User(user_event) => self.user_response(user_event),

            eng::NodeResponse::ConnectEventStarted(_node_id, _parameter_id) => {}
            eng::NodeResponse::ConnectEventEnded { input: input_id, output: output_id } => {
                let input_search = self.user_state.graph_state.arg_mapping
                    .find_by_input_id(input_id);
                let output_search = self.user_state.graph_state.arg_mapping
                    .find_by_output_id(output_id);

                match (input_search, output_search) {
                    (FindByInputIdResult::Input(input_arg_address),
                        FindByOutputIdResult::Output(output_arg_address)) => {
                        let input_node = self.user_state.graph_state.graph
                            .node_by_id_mut(input_arg_address.node_id)
                            .unwrap();

                        input_node.inputs[input_arg_address.index as usize].binding =
                            Binding::Output(OutputBinding {
                                output_node_id: output_arg_address.node_id,
                                output_index: output_arg_address.index,
                            });
                    }
                    (FindByInputIdResult::Trigger(node_id),
                        FindByOutputIdResult::Event(event_address)) => {
                        let event_node = self.user_state.graph_state.graph
                            .node_by_id_mut(event_address.node_id)
                            .unwrap();
                        event_node.events[event_address.index as usize].subscribers.push(node_id);
                    }
                    _ => panic!("Invalid connection")
                }
            }
            eng::NodeResponse::CreatedNode(node_id) => {
                let eng_node = &mut self.state.graph.nodes[node_id];
                let function = self.user_state.functions
                    .iter()
                    .find(|function| function.self_id == eng_node.user_data.function_id)
                    .unwrap();
                let node = Node::from_function(function);
                eng_node.user_data.node_id = node.id();

                let arg_mapping = &mut self.user_state.graph_state.arg_mapping;
                let editor_node = &mut eng_node.user_data;
                register_node(editor_node, arg_mapping);

                self.user_state.graph_state.graph.add_node(node);
            }
            eng::NodeResponse::SelectNode(node_id) => {
                let eng_node = &mut self.state.graph.nodes[node_id];
                let node = self.user_state.graph_state.graph.node_by_id_mut(eng_node.user_data.node_id).unwrap();
                assert_eq!(node.inputs.len(), eng_node.user_data.inputs.len());
                assert_eq!(node.outputs.len(), eng_node.user_data.outputs.len());
            }
            eng::NodeResponse::DeleteNodeUi(_node_id) => {}
            eng::NodeResponse::DeleteNodeFull { node_id: _node_id, node } => {
                self.user_state.graph_state.graph.remove_node_by_id(node.user_data.node_id);
            }
            eng::NodeResponse::DisconnectEvent { input: input_id, output: output_id } => {
                let input_search = self.user_state.graph_state.arg_mapping
                    .find_by_input_id(input_id);
                let output_search = self.user_state.graph_state.arg_mapping
                    .find_by_output_id(output_id);
                match (input_search, output_search) {
                    (FindByInputIdResult::Input(input_arg_address),
                        FindByOutputIdResult::Output(_output_arg_address)) => {
                        let input_node = self.user_state.graph_state.graph
                            .node_by_id_mut(input_arg_address.node_id)
                            .unwrap();

                        let input = &mut input_node.inputs[input_arg_address.index as usize];
                        if input.const_value.is_some() {
                            input.binding = Binding::Const;
                        } else {
                            input.binding = Binding::None;
                        }
                    }
                    (FindByInputIdResult::Trigger(node_id),
                        FindByOutputIdResult::Event(event_address)) => {
                        let event_node = self.user_state.graph_state.graph
                            .node_by_id_mut(event_address.node_id)
                            .unwrap();
                        let event = &mut event_node.events[event_address.index as usize];
                        event.subscribers.retain(|subscriber| *subscriber != node_id);
                    }
                    _ => panic!("Invalid connection")
                }
            }
            eng::NodeResponse::RaiseNode(_node_id) => {}
            eng::NodeResponse::MoveNode { node: _node_id, drag_delta: _delta } => {}
        }
    }
}

fn load_invokers(logger: Logger, out_funcs: &mut Vec<Function>) -> Vec<Box<dyn Invoker>> {
    let basic_invoker = BasicInvoker::new(logger);
    let timers_invoker = TimersInvoker::default();

    out_funcs.extend(basic_invoker.all_functions());
    out_funcs.extend(timers_invoker.all_functions());

    vec![
        Box::new(basic_invoker),
        Box::new(timers_invoker),
    ]
}