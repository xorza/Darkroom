use std::collections::{HashMap, HashSet};
use std::time::Instant;

use anyhow::anyhow;
use uuid::Uuid;

use crate::common::is_debug;
use crate::graph::*;

#[derive(Default)]
pub struct Preprocess {}

#[derive(Default, Clone)]
pub struct PreprocessOutput {
    pub connection_count: u32,
}

#[derive(Clone)]
pub struct PreprocessNode {
    node_id: NodeId,

    pub name: String,
    pub outputs: Vec<PreprocessOutput>,

    pub is_output: bool,

    pub behavior: FunctionBehavior,
    pub has_missing_inputs: bool,
    pub has_outputs: bool,

    pub should_execute: bool,
    pub execution_index: Option<u32>,
}

#[derive(Default, Clone)]
struct PreprocessInput {
    output_node_id: NodeId,
    output_index: u32,
    input_node_id: NodeId,
    input_index: u32,
    has_missing_inputs: bool,
    connection_behavior: BindingBehavior,
    is_output: bool,
}

#[derive(Default)]
pub struct PreprocessInfo {
    pub nodes: Vec<PreprocessNode>,
}

impl Preprocess {
    pub fn run(&self, graph: &Graph, prev_run: &PreprocessInfo) -> anyhow::Result<PreprocessInfo> {
        assert!(graph.validate().is_ok());

        let pp_inputs = self.collect_all_inputs(graph)?;
        let preprocess_info = self.gathepp_inputs_to_runtime(graph, pp_inputs, prev_run);
        let preprocess_info = self.mark_active_and_missing_inputs(graph, preprocess_info);
        let exec_order = self.create_exec_order(graph, &preprocess_info);
        let preprocess_info = self.execute(graph, preprocess_info, exec_order)?;

        Ok(preprocess_info)
    }

    fn collect_all_inputs(&self, graph: &Graph) -> anyhow::Result<Vec<PreprocessInput>> {
        let mut input_bindings = graph.nodes()
            .iter()
            .filter(|node| node.is_output)
            .map(|node| {
                PreprocessInput {
                    output_node_id: node.id(),
                    output_index: 0,
                    input_node_id: NodeId::nil(),
                    input_index: 0,
                    has_missing_inputs: false,
                    connection_behavior: BindingBehavior::Always,
                    is_output: true,
                }
            })
            .collect::<Vec<PreprocessInput>>();

        let mut node_ids: HashSet<NodeId> = HashSet::new();

        let mut i: usize = 0;
        while i < input_bindings.len() {
            i += 1;
            let i = i - 1;

            let input_binding = &input_bindings[i];
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

                        input_bindings.push(PreprocessInput {
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

            input_bindings[i].has_missing_inputs = has_missing_inputs;
        }

        Ok(input_bindings)
    }
    fn gathepp_inputs_to_runtime(
        &self,
        graph: &Graph,
        all_pp_inputs: Vec<PreprocessInput>,
        prev_run: &PreprocessInfo)
        -> PreprocessInfo
    {
        let mut pp_nodes = PreprocessInfo::default();
        let mut node_ids: HashSet<NodeId> = HashSet::new();

        for pp_input in all_pp_inputs.iter().rev() {
            let node_id = pp_input.output_node_id;
            if !node_ids.insert(node_id) {
                continue;
            }

            let node = graph.node_by_id(node_id).unwrap();

            let pp_inputs = all_pp_inputs.iter()
                .filter(|node| node.output_node_id == node_id)
                .collect::<Vec<&PreprocessInput>>();

            let has_missing_inputs = pp_inputs.iter()
                .any(|pp_input| pp_input.has_missing_inputs);

            let has_outputs = prev_run.nodes.iter()
                .any(|pp_node| pp_node.node_id == node_id && pp_node.has_outputs);

            let is_output = pp_inputs.iter()
                .any(|pp_input| pp_input.is_output);

            pp_nodes.nodes.push(PreprocessNode {
                node_id,
                has_missing_inputs,
                outputs: vec![PreprocessOutput::default(); node.outputs.len()],
                has_outputs,
                should_execute: false,
                execution_index: None,
                is_output,
                behavior: node.behavior,
                name: node.name.clone(),
            });
        }

        pp_nodes
    }
    fn mark_active_and_missing_inputs(&self, graph: &Graph, mut pp_nodes: PreprocessInfo) -> PreprocessInfo {
        for i in 0..pp_nodes.nodes.len() {
            let node_id = pp_nodes.nodes[i].node_id;
            let has_arguments = pp_nodes.nodes[i].has_outputs;
            let mut has_missing_inputs = pp_nodes.nodes[i].has_missing_inputs;
            let mut behavior = pp_nodes.nodes[i].behavior;

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
                            let output_r_node = pp_nodes
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

            pp_nodes.nodes[i].behavior = behavior;
            pp_nodes.nodes[i].has_missing_inputs = has_missing_inputs;
        }

        pp_nodes
    }
    fn create_exec_order(&self, graph: &Graph, pp_nodes: &PreprocessInfo) -> Vec<NodeId> {
        let mut exec_order = pp_nodes.nodes.iter()
            .rev()
            .filter(|r_node| r_node.is_output && !r_node.has_missing_inputs)
            .map(|r_node| r_node.node_id)
            .collect::<Vec<NodeId>>();

        let mut i: usize = 0;
        while i < exec_order.len() {
            i += 1;
            let i = i - 1;

            let node_id = exec_order[i];
            let node = graph.node_by_id(node_id).unwrap();
            let r_node = pp_nodes.node_by_id(node_id);

            if !r_node.has_outputs || r_node.behavior == FunctionBehavior::Active {
                for (_index, input) in node.inputs.iter().enumerate() {
                    match &input.binding {
                        Binding::None => { panic!("Missing input") }
                        Binding::Const => {}
                        Binding::Output(output_binding) => {
                            let r_output_node = pp_nodes
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
        &self,
        graph: &Graph,
        mut pp_nodes: PreprocessInfo,
        order: Vec<NodeId>,
    ) -> anyhow::Result<PreprocessInfo>
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

                        let output_r_node = pp_nodes
                            .node_by_id_mut(output_binding.output_node_id);

                        assert!(output_r_node.has_outputs);

                        if output_r_node.should_execute {
                            output_r_node.outputs[output_binding.output_index as usize].connection_count += 1;
                        }
                    }
                }
            }

            let r_node = pp_nodes.nodes
                .iter_mut()
                .find(|rnode| rnode.node_id == node_id)
                .unwrap();

            r_node.has_outputs = true;
            r_node.should_execute = true;
            r_node.execution_index = Some(execution_index);
        }

        pp_nodes.nodes.sort_by_key(|r_node| r_node.execution_index.unwrap_or(u32::MAX));

        Ok(pp_nodes)
    }
}

impl PreprocessNode {
    pub fn node_id(&self) -> NodeId {
        self.node_id
    }
}

impl PreprocessInfo {
    pub fn node_by_name(&self, name: &str) -> Option<&PreprocessNode> {
        self.nodes.iter().find(|node| node.name == name)
    }

    pub fn node_by_id(&self, node_id: NodeId) -> &PreprocessNode {
        self.nodes.iter()
            .find(|node| node.node_id == node_id)
            .unwrap()
    }
    pub fn node_by_id_mut(&mut self, node_id: NodeId) -> &mut PreprocessNode {
        self.nodes.iter_mut()
            .find(|node| node.node_id == node_id)
            .unwrap()
    }
}
