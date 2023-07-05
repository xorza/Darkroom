use std::collections::{HashMap, HashSet};
use std::time::Instant;

use anyhow::anyhow;
use uuid::Uuid;

use crate::common::is_debug;
use crate::graph::*;

#[derive(Default)]
pub struct Runtime {}

#[derive(Default, Clone)]
pub struct RuntimeOutput {
    pub connection_count: u32,
}

#[derive(Clone)]
pub struct RuntimeNode {
    node_id: Uuid,

    pub name: String,
    pub outputs: Vec<RuntimeOutput>,

    pub is_output: bool,

    pub behavior: FunctionBehavior,
    pub has_missing_inputs: bool,
    pub has_outputs: bool,

    pub should_execute: bool,
    pub execution_index: Option<u32>,
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
pub struct RuntimeInfo {
    pub nodes: Vec<RuntimeNode>,
}

impl Runtime {
    pub fn run(&mut self, graph: &Graph, previous_run: &RuntimeInfo) -> anyhow::Result<RuntimeInfo> {
        assert!(graph.validate().is_ok());

        let r_inputs = self.collect_all_inputs(graph)?;
        let runtime_info = self.gather_inputs_to_runtime(graph, r_inputs, previous_run);
        let runtime_info = self.mark_active_and_missing_inputs(graph, runtime_info);
        let exec_order = self.create_exec_order(graph, &runtime_info);
        let runtime_info = self.execute(graph, runtime_info, exec_order)?;

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

            let input_binding = &inputs_bindings[i];
            if !node_ids.insert(input_binding.output_node_id) {
                continue;
            }

            let mut has_missing_inputs = false;
            let node = graph
                .node_by_id(input_binding.output_node_id)
                .ok_or(anyhow!("Node not found"))?;
            for (input_index, input) in node.inputs.iter().enumerate() {
                match &input.binding {
                    Binding::None => {
                        has_missing_inputs |= input.is_required;
                    }
                    Binding::Const => {}
                    Binding::Output(output_binding) => {
                        assert_ne!(output_binding.output_node_id, node.id());

                        inputs_bindings.push(RuntimeInput {
                            output_node_id: output_binding.output_node_id,
                            output_index: output_binding.output_index,
                            input_node_id: node.id(),
                            input_index: input_index as u32,
                            has_missing_inputs: false,
                            connection_behavior: output_binding.behavior,
                            is_output: false,
                        });
                    }
                }
            }

            inputs_bindings[i].has_missing_inputs = has_missing_inputs;
        }

        Ok(inputs_bindings)
    }
    fn gather_inputs_to_runtime(
        &self,
        graph: &Graph,
        all_r_inputs: Vec<RuntimeInput>,
        previous_run: &RuntimeInfo)
        -> RuntimeInfo
    {
        let mut r_nodes = RuntimeInfo::default();
        let mut node_ids: HashSet<Uuid> = HashSet::new();

        for r_input in all_r_inputs.iter().rev() {
            let node_id = r_input.output_node_id;
            if !node_ids.insert(node_id) {
                continue;
            }

            let node = graph.node_by_id(node_id).unwrap();

            let r_inputs = all_r_inputs.iter()
                .filter(|node| node.output_node_id == node_id)
                .collect::<Vec<&RuntimeInput>>();

            let has_missing_inputs = r_inputs.iter()
                .any(|node| node.has_missing_inputs);

            let r_outputs: Vec<RuntimeOutput> = vec![RuntimeOutput::default(); node.outputs.len()];

            let has_outputs = previous_run.nodes.iter()
                .any(|node| node.node_id == node_id && node.has_outputs);

            let is_output = r_inputs.iter()
                .any(|r_input| r_input.is_output);

            r_nodes.nodes.push(RuntimeNode {
                node_id,
                has_missing_inputs,
                outputs: r_outputs,
                has_outputs,
                should_execute: false,
                execution_index: None,
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
            let has_arguments = r_nodes.nodes[i].has_outputs;
            let mut has_missing_inputs = r_nodes.nodes[i].has_missing_inputs;
            let mut behavior = r_nodes.nodes[i].behavior;

            let node = graph.node_by_id(node_id).unwrap();

            if !has_arguments {
                behavior = FunctionBehavior::Active;
            }
            if behavior != FunctionBehavior::Active {
                for input in node.inputs.iter() {
                    match &input.binding {
                        Binding::None => { panic!("Missing input") }
                        Binding::Const => {}
                        Binding::Output(output_binding) => {
                            let output_r_node = r_nodes
                                .node_by_id(output_binding.output_node_id);

                            has_missing_inputs |= output_r_node.has_missing_inputs;

                            if output_binding.behavior == BindingBehavior::Always
                                && output_r_node.behavior == FunctionBehavior::Active {
                                behavior = FunctionBehavior::Active;
                            }
                        }
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
            let r_node = r_nodes.node_by_id(node_id);

            if !r_node.has_outputs || r_node.behavior == FunctionBehavior::Active {
                for (_index, input) in node.inputs.iter().enumerate() {
                    match &input.binding {
                        Binding::None => { panic!("Missing input") }
                        Binding::Const => {}
                        Binding::Output(output_binding) => {
                            let r_output_node = r_nodes
                                .node_by_id(output_binding.output_node_id);

                            assert!(!r_output_node.has_missing_inputs);

                            if r_output_node.behavior == FunctionBehavior::Active {
                                exec_order.push(output_binding.output_node_id);
                            }
                        }
                    }
                }
            }
        }

        exec_order.reverse();
        exec_order
    }
    fn execute(
        &mut self,
        graph: &Graph,
        mut r_nodes: RuntimeInfo,
        order: Vec<Uuid>,
    ) -> anyhow::Result<RuntimeInfo>
    {
        for (execution_index, i) in (0_u32..).zip(0..order.len()) {
            let node_id = order[i];
            let node = graph.node_by_id(node_id).unwrap();

            for (_index, input) in node.inputs.iter().enumerate() {
                match &input.binding {
                    Binding::None => { panic!("Missing input") }
                    Binding::Const => {}
                    Binding::Output(output_binding) => {
                        assert_ne!(output_binding.output_node_id, node.id());

                        let output_r_node = r_nodes
                            .node_by_id_mut(output_binding.output_node_id);

                        assert!(output_r_node.has_outputs);

                        if output_r_node.should_execute {
                            output_r_node.outputs[output_binding.output_index as usize].connection_count += 1;
                        }
                    }
                }
            }

            let r_node = r_nodes.nodes
                .iter_mut()
                .find(|rnode| rnode.node_id == node_id)
                .unwrap();

            r_node.has_outputs = true;
            r_node.should_execute = true;
            r_node.execution_index = Some(execution_index);
        }

        r_nodes.nodes.sort_by_key(|r_node| r_node.execution_index.unwrap_or(u32::MAX));

        Ok(r_nodes)
    }
}

impl RuntimeNode {
    pub fn node_id(&self) -> Uuid {
        self.node_id
    }
}

impl RuntimeInfo {
    pub fn node_by_name(&self, name: &str) -> Option<&RuntimeNode> {
        self.nodes.iter().find(|node| node.name == name)
    }

    pub fn node_by_id(&self, node_id: Uuid) -> &RuntimeNode {
        self.nodes.iter()
            .find(|node| node.node_id == node_id)
            .unwrap()
    }
    pub fn node_by_id_mut(&mut self, node_id: Uuid) -> &mut RuntimeNode {
        self.nodes.iter_mut()
            .find(|node| node.node_id == node_id)
            .unwrap()
    }
}