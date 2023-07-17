use std::collections::HashMap;

use imaginarium::wgpu::wgpu_context::WgpuContext;

use crate::function::{Function, FunctionId};
use crate::invoke::{InvokeArgs, Invoker};
use crate::runtime_graph::InvokeContext;

pub trait WgpuInvokable {
    fn new(wgpu_context: &WgpuContext) -> Self where Self: Sized;
    fn descriptor(&self) -> Function;
    fn invoke(
        &self,
        wgpu_context: &WgpuContext,
        invoke_context: &mut InvokeContext,
        inputs: &InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()>;
}


pub(crate) struct WgpuInvoker {
    context: WgpuContext,
    funcs: HashMap<FunctionId, Box<dyn WgpuInvokable>>,
}

impl WgpuInvoker {
    pub fn add_function<T>(&mut self)
    where T: WgpuInvokable + 'static
    {
        let wgpu_func = T::new(&self.context);
        let boxed_wgpu_func = Box::new(wgpu_func);
        let func_id = boxed_wgpu_func.descriptor().self_id;
        self.funcs.insert(func_id, boxed_wgpu_func);
    }
}

impl Invoker for WgpuInvoker {
    fn all_functions(&self) -> Vec<Function> {
        self.funcs
            .values()
            .map(|f| f.descriptor())
            .collect()
    }

    fn invoke(
        &self,
        function_id: FunctionId,
        ctx: &mut InvokeContext,
        inputs: &InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()>
    {
        let invokable = self.funcs.get(&function_id).unwrap();
        invokable.invoke(&self.context, ctx, inputs, outputs)?;

        Ok(())
    }
}

