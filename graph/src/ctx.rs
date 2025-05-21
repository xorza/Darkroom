use crate::compute::Compute;
use crate::function::FuncLib;
use crate::graph::{Graph, NodeId};
use crate::invoke::UberInvoker;
use crate::runtime_graph::RuntimeGraph;
use crate::worker::Worker;

#[repr(u32)]
pub enum CallbackType {
    OnGraphUpdate,
    OnFuncLibUpdate,
}

pub type CallbackDelegate = dyn Fn(CallbackType);

#[derive(Default)]
pub struct Context {
    pub graph: Graph,
    pub runtime_graph: RuntimeGraph,
    pub compute: Compute,
    pub invoker: UberInvoker,
    pub func_lib: FuncLib,

    pub worker: Option<Worker>,

    pub callback: Option<Box<CallbackDelegate>>,
}

impl Context {
    pub fn set_output_binding(
        &mut self,
        output_node_id: NodeId,
        output_index: u32,
        input_node_id: NodeId,
        input_index: u32,
    ) -> anyhow::Result<()> {
        let output_data_type = {
            let output_node = self
                .graph
                .node_by_id(output_node_id)
                .ok_or(anyhow::anyhow!("Output node not found"))?;
            let output_func = self
                .func_lib
                .func_by_id(output_node.func_id)
                .ok_or(anyhow::anyhow!("Output function not found"))?;

            if output_func.outputs.len() <= output_index as usize {
                return Err(anyhow::anyhow!("Output index out of bounds"));
            }

            &output_func.outputs[output_index as usize].data_type
        };

        let input_node = self.graph.node_by_id_mut(input_node_id).unwrap();
        let input_func = self.func_lib.func_by_id(input_node.func_id).unwrap();

        if input_func.inputs.len() <= input_index as usize {
            return Err(anyhow::anyhow!("Input index out of bounds"));
        }
        if input_func.inputs[input_index as usize].data_type != *output_data_type {
            return Err(anyhow::anyhow!("Data types do not match"));
        }
        input_node.inputs[input_index as usize].binding =
            crate::graph::Binding::Output(crate::graph::OutputBinding {
                output_node_id,
                output_index,
            });

        Ok(())
    }
}
