use imaginarium::wgpu::wgpu_context::WgpuContext;

use crate::functions::{Function, FunctionId};
use crate::invoke::{InvokeArgs, Invoker};
use crate::runtime_graph::InvokeContext;

struct WgpuInvoker {
    context: WgpuContext,
}

impl WgpuInvoker {
    pub fn new() -> anyhow::Result<Self> {
        let context = WgpuContext::new()?;

        Ok(Self {
            context,
        })
    }
}

impl Invoker for WgpuInvoker {
    fn all_functions(&self) -> Vec<Function> {
        todo!()
    }

    fn invoke(
        &self,
        _function_id: FunctionId,
        _ctx: &mut InvokeContext,
        _inputs: &InvokeArgs,
        _outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()>
    {
        Ok(())
    }
}

