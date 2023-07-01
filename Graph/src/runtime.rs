use std::collections::{HashMap, HashSet};
use std::time::Instant;

use anyhow::anyhow;
use uuid::Uuid;

use crate::common::is_debug;
use crate::data::Value;
use crate::graph::*;
use crate::invoke::{Args, Invoker};

#[derive(Default)]
pub struct Runtime {
    arg_cache: HashMap<Uuid, ArgSet>,
}

#[derive(Default, Clone)]
pub struct RuntimeOutput {
    connection_count: u32,
    behavior: BindingBehavior,
}

#[derive(Clone)]
pub struct RuntimeNode {
    node_id: Uuid,

    pub name: String,
    pub outputs: Vec<RuntimeOutput>,

    pub is_output: bool,

    pub behavior: NodeBehavior,
    pub has_missing_inputs: bool,
    pub has_arguments: bool,

    pub executed: bool,
    pub execution_index: u32,
    pub run_time: f64,
}

#[derive(Default, Clone)]
struct RuntimeInput {
    output_node_id: Uuid,
    output_index: u32,
    input_node_id: Uuid,
    input_index: u32,
    has_missing_inputs: bool,
    connection_behavior: BindingBehavior,
    is_output: bool,
}

#[derive(Default)]
struct ArgSet {
    inputs: Args,
    outputs: Args,
    node_id: Uuid,
}

#[derive(Default)]
pub struct RuntimeInfo {
    pub nodes: Vec<RuntimeNode>,
}

impl Runtime {
    pub fn run(&mut self, graph: &Graph, invoker: &dyn Invoker) -> anyhow::Result<RuntimeInfo> {
        assert!(graph.validate().is_ok());

        let r_inputs = self.collect_all_inputs(graph)?;
        let r_nodes = self.gather_inputs_to_runtime(graph, r_inputs);
        let r_nodes = self.mark_active_and_missing_inputs(graph, r_nodes);
        let exec_order = self.create_exec_order(graph, &r_nodes);
        let runtime_info = self.execute(graph, r_nodes, exec_order, invoker)?;

        Ok(runtime_info)
    }

    fn collect_all_inputs(&self, graph: &Graph) -> anyhow::Result<Vec<RuntimeInput>> {
        let mut inputs_bindings
            = graph.nodes().iter()
            .filter(|node| node.is_output)
            .map(|node| {
                RuntimeInput {
                    output_node_id: node.id(),
                    output_index: 0,
                    input_node_id: Uuid::nil(),
                    input_index: 0,
                    has_missing_inputs: false,
                    connection_behavior: BindingBehavior::Always,
                    is_output: true,
                }
            })
            .collect::<Vec<RuntimeInput>>();

        let mut node_ids: HashSet<Uuid> = HashSet::new();

        let mut i: usize = 0;
        while i < inputs_bindings.len() {
            i += 1;
            let i = i - 1;

            let node_input_binding = &inputs_bindings[i];
            if !node_ids.insert(node_input_binding.output_node_id) {
                continue;
            }

            let mut has_missing_inputs = false;
            let node = graph.node_by_id(node_input_binding.output_node_id).ok_or(anyhow!("Node not found"))?;
            for (input_index, input) in node.inputs.iter().enumerate() {
                if input.binding.is_some() {
                    let binding = input.binding.as_ref().unwrap();
                    assert_ne!(binding.output_node_id(), node.id());

                    inputs_bindings.push(RuntimeInput {
                        output_node_id: binding.output_node_id(),
                        output_index: binding.output_index(),
                        input_node_id: binding.output_node_id(),
                        input_index: input_index as u32,
                        has_missing_inputs: false,
                        connection_behavior: binding.behavior,
                        is_output: false,
                    });
                } else if input.default_value.is_some() {} else {
                    has_missing_inputs |= input.is_required;
                }
            }

            inputs_bindings[i].has_missing_inputs = has_missing_inputs;
        }

        Ok(inputs_bindings)
    }
    fn gather_inputs_to_runtime(&self, graph: &Graph, r_inputs: Vec<RuntimeInput>) -> RuntimeInfo {
        let mut r_nodes = RuntimeInfo::default();
        let mut node_ids: HashSet<Uuid> = HashSet::new();

        for r_input in r_inputs.iter().rev() {
            let output_node_id = r_input.output_node_id;
            if !node_ids.insert(output_node_id) {
                continue;
            }

            let node = graph.node_by_id(output_node_id).unwrap();

            let output_node_r_inputs = r_inputs.iter()
                .filter(|node| node.output_node_id == output_node_id)
                .collect::<Vec<&RuntimeInput>>();

            let has_missing_inputs = output_node_r_inputs.iter()
                .any(|node| node.has_missing_inputs);

            let mut r_outputs: Vec<RuntimeOutput> = Vec::new();
            r_outputs.resize(node.outputs.len(), RuntimeOutput::new());

            if !r_outputs.is_empty() {
                for edge in output_node_r_inputs.iter() {
                    let r_output = &mut r_outputs[edge.output_index as usize];
                    r_output.connection_count += 1;
                    r_output.behavior = edge.connection_behavior;
                }
            }

            let has_outputs = self.arg_cache.contains_key(&output_node_id);
            let is_output = node.is_output || output_node_r_inputs.iter().any(|r_input| r_input.is_output);

            r_nodes.nodes.push(RuntimeNode {
                node_id: output_node_id,
                has_missing_inputs,
                outputs: r_outputs,
                has_arguments: has_outputs,
                executed: false,
                execution_index: 0,
                run_time: 0.0,
                is_output,
                behavior: node.behavior,
                name: node.name.clone(),
            });
        }

        r_nodes
    }
    fn mark_active_and_missing_inputs(&self, graph: &Graph, mut r_nodes: RuntimeInfo) -> RuntimeInfo {
        for i in 0..r_nodes.nodes.len() {
            let node_id = r_nodes.nodes[i].node_id;
            let has_arguments = r_nodes.nodes[i].has_arguments;
            let mut has_missing_inputs = r_nodes.nodes[i].has_missing_inputs;
            let mut behavior = r_nodes.nodes[i].behavior;

            let node = graph.node_by_id(node_id).unwrap();

            if !has_arguments {
                behavior = NodeBehavior::Active;
            }
            if behavior != NodeBehavior::Active {
                for input in node.inputs.iter() {
                    let binding = input.binding.as_ref().unwrap();
                    let output_r_node = r_nodes.node_by_id(binding.output_node_id()).unwrap();

                    has_missing_inputs |= output_r_node.has_missing_inputs;

                    if binding.behavior == BindingBehavior::Always
                        && output_r_node.behavior == NodeBehavior::Active {
                        behavior = NodeBehavior::Active;
                    }
                }
            }

            r_nodes.nodes[i].behavior = behavior;
            r_nodes.nodes[i].has_missing_inputs = has_missing_inputs;
        }

        r_nodes
    }
    fn create_exec_order(&self, graph: &Graph, r_nodes: &RuntimeInfo) -> Vec<Uuid> {
        let mut exec_order = r_nodes.nodes.iter()
            .rev()
            .filter(|r_node| r_node.is_output && !r_node.has_missing_inputs)
            .map(|r_node| r_node.node_id)
            .collect::<Vec<Uuid>>();

        let mut i: usize = 0;
        while i < exec_order.len() {
            i += 1;
            let i = i - 1;

            let node_id = exec_order[i];
            let node = graph.node_by_id(node_id).unwrap();
            let r_node = r_nodes.node_by_id(node_id).unwrap();

            if !r_node.has_arguments || r_node.behavior == NodeBehavior::Active {
                for (_, input) in node.inputs.iter().enumerate() {
                    if let Some(binding) = input.binding.as_ref() {
                        let r_output_node = r_nodes.node_by_id(binding.output_node_id()).unwrap();

                        assert!(!r_output_node.has_missing_inputs);

                        if r_output_node.behavior == NodeBehavior::Active {
                            exec_order.push(binding.output_node_id());
                        }
                    }
                }
            }
        }

        exec_order.reverse();
        exec_order
    }
    fn execute(&mut self, graph: &Graph, mut r_nodes: RuntimeInfo, order: Vec<Uuid>, invoker: &dyn Invoker) -> anyhow::Result<RuntimeInfo> {
        invoker.start();

        for (execution_index, i) in (0_u32..).zip(0..order.len()) {
            let node_id = order[i];
            let node = graph.node_by_id(node_id).unwrap();

            let mut input_args = match self.arg_cache.remove(&node_id) {
                Some(value) => value,
                None => { ArgSet::from_node(node) }
            };

            for (input_index, input) in node.inputs.iter().enumerate() {
                if let Some(binding) = input.binding.as_ref() {
                    assert_ne!(binding.output_node_id(), node.id());

                    let output_r_node = r_nodes.node_by_id(binding.output_node_id()).unwrap();

                    assert!(output_r_node.has_arguments);

                    if output_r_node.executed {
                        let output_args = self.arg_cache.get(&binding.output_node_id()).unwrap();

                        if is_debug() {
                            let output_value = &output_args.outputs[binding.output_index() as usize];
                            assert_eq!(input.data_type, output_value.data_type());

                            let output_node = graph.node_by_id(binding.output_node_id()).unwrap();
                            let output = &output_node.outputs[binding.output_index() as usize];
                            assert_eq!(input.data_type, output.data_type);
                        }

                        input_args.inputs[input_index] = output_args.outputs[binding.output_index() as usize].clone();
                    }
                } else if let Some(value) = input.default_value.as_ref() {
                    input_args.inputs[input_index] = value.clone();
                } else {
                    panic!("Missing input value for node: {}", node.name);
                }
            }

            let start = Instant::now();
            invoker.call(node.function_id, node_id, &input_args.inputs, &mut input_args.outputs)?;

            let rnode = r_nodes.nodes.iter_mut().find(|rnode| rnode.node_id == node_id).unwrap();
            rnode.run_time = start.elapsed().as_secs_f64();
            rnode.has_arguments = true;
            rnode.execution_index = execution_index;
            rnode.executed = true;

            self.arg_cache.insert(node_id, input_args);
        }

        invoker.finish();

        Ok(r_nodes)
    }
}

impl RuntimeNode {
    pub fn node_id(&self) -> Uuid {
        self.node_id
    }
}

impl ArgSet {
    pub fn from_node(node: &Node) -> ArgSet {
        let inputs: Args = Args::with_size(node.inputs.len());
        let mut outputs: Args = Args::with_size(node.outputs.len());
        for (i, output) in node.outputs.iter().enumerate() {
            outputs[i] = Value::from(output.data_type);
        }

        ArgSet {
            node_id: node.id(),
            inputs,
            outputs,
        }
    }
}

impl RuntimeOutput {
    pub fn new() -> RuntimeOutput {
        RuntimeOutput {
            behavior: BindingBehavior::Once,
            ..Self::default()
        }
    }
}

impl RuntimeInfo {
    pub fn node_by_name(&self, name: &str) -> Option<&RuntimeNode> {
        self.nodes.iter().find(|node| node.name == name)
    }

    pub fn node_by_id(&self, node_id: Uuid) -> Option<&RuntimeNode> {
        self.nodes.iter().find(|node| node.node_id == node_id)
    }
}