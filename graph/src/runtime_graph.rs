use std::mem::take;

use anyhow::Result;
use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

use crate::data::DynamicValue;
use crate::function::FuncLib;
use crate::graph::{Binding, Graph, Node, NodeBehavior, NodeId};
use crate::invoke::InvokeCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum ProcessingState {
    #[default]
    None,
    Processing,
    Processed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RuntimePortAddress {
    pub r_node_idx: usize,
    pub port_idx: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum InputState {
    #[default]
    Unknown,
    Unchanged,
    Changed,
    Missing,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RuntimeInput {
    state: InputState,
    required: bool,
    output_address: Option<RuntimePortAddress>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RuntimeOutput {
    #[default]
    Unused,
    Used,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct RuntimeNode {
    pub id: NodeId,

    pub has_missing_inputs: bool,
    pub has_changed_inputs: bool,
    pub should_invoke: bool,
    pub invocation_order: usize,

    processing_state: ProcessingState,
    node_idx: usize,
    pub inputs: Vec<RuntimeInput>,
    pub outputs: Vec<RuntimeOutput>,

    pub run_time: f64,

    #[serde(skip)]
    pub(crate) cache: InvokeCache,
    #[serde(skip)]
    pub(crate) output_values: Option<Vec<DynamicValue>>,

    #[cfg(debug_assertions)]
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RuntimeGraph {
    pub r_nodes: Vec<RuntimeNode>,
}

impl RuntimeNode {
    fn reset_from(&mut self, node: &Node) {
        let mut prev_state = take(self);

        self.id = node.id;

        self.inputs = take(&mut prev_state.inputs);
        self.inputs.fill(RuntimeInput::default());
        self.inputs
            .resize(node.inputs.len(), RuntimeInput::default());

        self.outputs = take(&mut prev_state.outputs);
        self.outputs.fill(RuntimeOutput::default());

        #[cfg(debug_assertions)]
        {
            self.name = take(&mut prev_state.name);
            self.name.clear();
            self.name.push_str(&node.name);
        }

        self.cache = take(&mut prev_state.cache);
        self.output_values = take(&mut prev_state.output_values);
    }
}

impl RuntimeGraph {
    pub fn node_by_id(&self, node_id: NodeId) -> Option<&RuntimeNode> {
        self.r_nodes.iter().find(|node| node.id == node_id)
    }
    pub fn node_by_id_mut(&mut self, node_id: NodeId) -> Option<&mut RuntimeNode> {
        self.r_nodes.iter_mut().find(|node| node.id == node_id)
    }

    // mark active nodes without cached outputs for execution
    pub fn next(&mut self, graph: &Graph, func_lib: &FuncLib) {
        self.build_node_cache(graph);
        self.build_active_node_indices_ordered(graph, func_lib);
        self.propagate_input_state(graph);
    }

    fn build_node_cache(&mut self, graph: &Graph) {
        for (node_idx, node) in graph.nodes.iter().enumerate() {
            assert!(
                !node.id.is_nil(),
                "Graph node has nil id at index {}",
                node_idx
            );

            let r_node_idx = self
                .r_nodes
                .iter()
                .position(|r_node| r_node.id == node.id)
                .unwrap_or_else(|| {
                    self.r_nodes.push(RuntimeNode::default());
                    self.r_nodes.len() - 1
                });

            let r_node = &mut self.r_nodes[r_node_idx];
            r_node.reset_from(node);
            r_node.node_idx = node_idx;
        }
    }

    fn build_active_node_indices_ordered(&mut self, graph: &Graph, func_lib: &FuncLib) {
        enum VisitCause {
            Terminal,
            OutputRequest { output_address: RuntimePortAddress },
            Processed,
        }
        struct Visit {
            r_node_idx: usize,
            cause: VisitCause,
        }
        let mut stack: Vec<Visit> = Vec::with_capacity(10);

        self.r_nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, r_node)| {
                if graph.nodes[r_node.node_idx].behavior == NodeBehavior::Terminal {
                    Some(idx)
                } else {
                    None
                }
            })
            .for_each(|idx| {
                stack.push(Visit {
                    r_node_idx: idx,
                    cause: VisitCause::Terminal,
                });
            });

        let mut invocation_order: usize = 0;
        while let Some(visit) = stack.pop() {
            let r_node_idx = visit.r_node_idx;
            let r_node = &mut self.r_nodes[r_node_idx];
            // println!("{}", r_node.name);

            let output_address = match visit.cause {
                VisitCause::Terminal => None,
                VisitCause::OutputRequest { output_address } => {
                    assert_eq!(
                        visit.r_node_idx, output_address.r_node_idx,
                        "Visit r_node_idx {} does not match output address r_node_idx {}",
                        visit.r_node_idx, output_address.r_node_idx
                    );
                    Some(output_address)
                }
                VisitCause::Processed => {
                    r_node.processing_state = ProcessingState::Processed;
                    r_node.invocation_order = invocation_order;
                    invocation_order += 1;
                    continue;
                }
            };

            match r_node.processing_state {
                ProcessingState::Processed => {
                    continue;
                }
                ProcessingState::Processing => {
                    // todo replace with result<>
                    panic!(
                        "Cycle detected while building runtime graph at node {:?}",
                        visit.r_node_idx
                    );
                }
                ProcessingState::None => {
                    let func = func_lib
                        .by_id(graph.nodes[r_node.node_idx].func_id)
                        .unwrap_or_else(|| todo!());
                    r_node
                        .inputs
                        .resize(func.inputs.len(), RuntimeInput::default());
                    func.inputs.iter().enumerate().for_each(|(idx, input)| {
                        r_node.inputs[idx] = RuntimeInput {
                            required: input.required,
                            ..Default::default()
                        };
                    });

                    r_node.outputs.fill(RuntimeOutput::Unused);
                    r_node
                        .outputs
                        .resize(func.outputs.len(), RuntimeOutput::default());

                    if let Some(output_address) = output_address {
                        r_node.outputs[output_address.port_idx] = RuntimeOutput::Used;
                    }
                }
            }

            r_node.processing_state = ProcessingState::Processing;
            stack.push(Visit {
                r_node_idx,
                cause: VisitCause::Processed,
            });

            for input in graph.nodes[r_node.node_idx].inputs.iter() {
                if let Binding::Output(output_binding) = &input.binding {
                    let output_r_node_idx = self
                        .r_nodes
                        .iter()
                        .position(|r_node| r_node.id == output_binding.output_node_id)
                        .unwrap_or_else(|| {
                            panic!(
                                "Runtime node index missing for node {:?}",
                                output_binding.output_node_id
                            )
                        });
                    stack.push(Visit {
                        r_node_idx: output_r_node_idx,
                        cause: VisitCause::OutputRequest {
                            output_address: RuntimePortAddress {
                                r_node_idx: output_r_node_idx,
                                port_idx: output_binding.output_idx,
                            },
                        },
                    });
                }
            }
        }
    }

    // mark missing inputs and propagate behavior based on upstream active nodes
    fn propagate_input_state(&mut self, graph: &Graph) {
        let mut active_node_indices = self
            .r_nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, r_node)| {
                if r_node.processing_state == ProcessingState::Processed {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        active_node_indices.sort_unstable_by_key(|&idx| self.r_nodes[idx].invocation_order);

        for r_node_idx in active_node_indices {
            let node = {
                let r_node = &mut self.r_nodes[r_node_idx];
                assert_eq!(
                    r_node.processing_state,
                    ProcessingState::Processed,
                    "Runtime node must be processed before input propagation"
                );
                // println!("{}", r_node.name);

                let node = &graph.nodes[r_node.node_idx];
                // avoid traversing inputs for NodeBehavior::Once nodes having outputs
                // even if having missing inputs
                if node.behavior == NodeBehavior::Once && r_node.output_values.is_some() {
                    // should_invoke is false
                    continue;
                }

                node
            };

            let mut has_changed_inputs = false;
            let mut has_missing_inputs = false;
            let mut output_address: Option<RuntimePortAddress> = None;

            for (input_idx, input) in node.inputs.iter().enumerate() {
                let input_state = match &input.binding {
                    Binding::None => InputState::Missing,
                    // todo implement notifying of const binding changes
                    Binding::Const => InputState::Changed,
                    Binding::Output(output_binding) => {
                        let output_r_node_idx = self
                            .r_nodes
                            .iter()
                            .position(|r_node| r_node.id == output_binding.output_node_id)
                            .unwrap_or_else(|| {
                                panic!(
                                    "Output binding references missing runtime node {:?}",
                                    output_binding.output_node_id
                                )
                            });
                        let output_r_node = &self.r_nodes[output_r_node_idx];

                        assert_eq!(
                            output_r_node.processing_state,
                            ProcessingState::Processed,
                            "Output runtime node {:?} not processed before input propagation",
                            output_binding.output_node_id
                        );

                        output_address = Some(RuntimePortAddress {
                            r_node_idx: output_r_node_idx,
                            port_idx: output_binding.output_idx,
                        });

                        if output_r_node.has_missing_inputs {
                            InputState::Missing
                        } else if output_r_node.should_invoke {
                            InputState::Changed
                        } else {
                            InputState::Unchanged
                        }
                    }
                };
                match input_state {
                    InputState::Unchanged => {}
                    InputState::Changed => has_changed_inputs = true,
                    InputState::Missing => {
                        has_missing_inputs |= self.r_nodes[r_node_idx].inputs[input_idx].required;
                    }
                    InputState::Unknown => panic!("unprocessed input"),
                }

                let runtime_input = &mut self.r_nodes[r_node_idx].inputs[input_idx];
                runtime_input.state = input_state;
                runtime_input.output_address = output_address;
            }

            let r_node = &mut self.r_nodes[r_node_idx];
            r_node.has_missing_inputs = has_missing_inputs;
            if !r_node.has_missing_inputs {
                match node.behavior {
                    NodeBehavior::Terminal | NodeBehavior::Always => {
                        r_node.should_invoke = true;
                    }
                    NodeBehavior::Once => {
                        // has no cached outputs, so should_invoke = true
                        r_node.should_invoke = true;
                    }
                    NodeBehavior::OnInputChange => {
                        r_node.should_invoke = has_changed_inputs;
                    }
                }
            }
        }
    }
}

fn validate_runtime_inputs(graph: &Graph, func_lib: &FuncLib) -> Result<()> {
    graph.validate()?;

    for node in graph.nodes.iter() {
        let func = func_lib.by_id(node.func_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "Missing function {:?} for node {:?}",
                node.func_id, node.id
            ))
        })?;

        if node.inputs.len() != func.inputs.len() {
            return Err(anyhow::Error::msg(format!(
                "Node {:?} input count mismatch",
                node.id
            )));
        }

        for input in node.inputs.iter() {
            if let Binding::Output(output_binding) = &input.binding {
                let output_node = graph
                    .node_by_id(output_binding.output_node_id)
                    .ok_or_else(|| anyhow::Error::msg("Output binding references missing node"))?;
                let output_func = func_lib.by_id(output_node.func_id).ok_or_else(|| {
                    anyhow::Error::msg(format!(
                        "Missing function {:?} for output node {:?}",
                        output_node.func_id, output_node.id
                    ))
                })?;
                if output_binding.output_idx >= output_func.outputs.len() {
                    return Err(anyhow::Error::msg(format!(
                        "Output index out of range for node {:?}",
                        output_binding.output_node_id
                    )));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_run() -> anyhow::Result<()> {
        let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
        let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

        let _get_b_node_id = graph
            .node_by_name("get_b")
            .unwrap_or_else(|| panic!("Node named \"get_b\" not found"))
            .id;

        let mut runtime_graph = RuntimeGraph::default();
        runtime_graph.next(&graph, &func_lib);

        assert_eq!(runtime_graph.r_nodes.len(), 5);
        assert!(runtime_graph
            .r_nodes
            .iter()
            .all(|r_node| r_node.should_invoke));
        assert!(runtime_graph
            .r_nodes
            .iter()
            .all(|r_node| !r_node.has_missing_inputs));

        let _yaml = serde_yml::to_string(&runtime_graph)?;

        Ok(())
    }

    #[test]
    fn empty_run() -> anyhow::Result<()> {
        let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
        let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

        let _get_b_node_id = graph
            .node_by_name("get_b")
            .unwrap_or_else(|| panic!("Node named \"get_b\" not found"))
            .id;

        let mut runtime_graph = RuntimeGraph::default();
        runtime_graph.next(&graph, &func_lib);

        assert_eq!(runtime_graph.r_nodes.len(), 5);

        runtime_graph.next(&graph, &func_lib);

        assert_eq!(runtime_graph.r_nodes.len(), 5);

        Ok(())
    }

    #[test]
    fn missing_input() -> anyhow::Result<()> {
        let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
        let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

        let get_b_node_id = graph
            .node_by_name("get_b")
            .unwrap_or_else(|| panic!("Node named \"get_b\" not found"))
            .id;
        let sum_node_id = graph
            .node_by_name("sum")
            .unwrap_or_else(|| panic!("Node named \"sum\" not found"))
            .id;
        let mult_node_id = graph
            .node_by_name("mult")
            .unwrap_or_else(|| panic!("Node named \"mult\" not found"))
            .id;
        let print_node_id = graph
            .node_by_name("print")
            .unwrap_or_else(|| panic!("Node named \"print\" not found"))
            .id;

        graph
            .node_by_name_mut("sum")
            .unwrap_or_else(|| panic!("Node named \"sum\" not found"))
            .inputs[0]
            .binding = Binding::None;

        let mut runtime_graph = RuntimeGraph::default();
        runtime_graph.next(&graph, &func_lib);

        assert_eq!(runtime_graph.r_nodes.len(), 5);
        assert!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", get_b_node_id))
                .should_invoke
        );
        assert!(
            !runtime_graph
                .node_by_id(sum_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", sum_node_id))
                .should_invoke
        );
        assert!(
            !runtime_graph
                .node_by_id(mult_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", mult_node_id))
                .should_invoke
        );
        assert!(
            !runtime_graph
                .node_by_id(print_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", print_node_id))
                .should_invoke
        );

        assert!(
            !runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", get_b_node_id))
                .has_missing_inputs
        );
        assert!(
            runtime_graph
                .node_by_id(sum_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", sum_node_id))
                .has_missing_inputs
        );
        assert!(
            runtime_graph
                .node_by_id(mult_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", mult_node_id))
                .has_missing_inputs
        );
        assert!(
            runtime_graph
                .node_by_id(print_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", print_node_id))
                .has_missing_inputs
        );

        let _yaml = serde_yml::to_string(&runtime_graph)?;

        Ok(())
    }
}
