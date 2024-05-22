use std::fmt;
use std::fmt::{Debug, Formatter};

use hashbrown::HashMap;

use imaginarium::wgpu::wgpu_context::WgpuContext;

use crate::function::{Func, FuncId, FuncLib};
use crate::invoke::{InvokeArgs, InvokeCache, Invoker};

pub trait WgpuInvokable: Send + Sync {
    fn new(wgpu_context: &WgpuContext) -> Self
    where
        Self: Sized;
    fn func(&self) -> Func;
    fn invoke(
        &self,
        wgpu_context: &WgpuContext,
        cache: &mut InvokeCache,
        inputs: &InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()>;
}

pub(crate) struct WgpuInvoker {
    context: WgpuContext,
    funcs: HashMap<FuncId, Box<dyn WgpuInvokable>>,
    func_lib: FuncLib,
}

impl WgpuInvoker {
    pub fn add_function<T>(&mut self)
    where
        T: WgpuInvokable + 'static,
    {
        let wgpu_func = T::new(&self.context);
        let boxed_wgpu_func = Box::new(wgpu_func);
        let func_id = boxed_wgpu_func.func().id;
        self.func_lib.add(boxed_wgpu_func.func());
        self.funcs.insert(func_id, boxed_wgpu_func);
    }
}

impl Invoker for WgpuInvoker {
    fn get_func_lib(&mut self) -> FuncLib {
        self.func_lib.clone()
    }

    fn invoke(
        &self,
        function_id: FuncId,
        ctx: &mut InvokeCache,
        inputs: &mut InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()> {
        let invokable = self.funcs.get(&function_id).unwrap();
        invokable.invoke(&self.context, ctx, inputs, outputs)?;

        Ok(())
    }
}

impl Debug for WgpuInvoker {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("WgpuInvoker")
            .field("context", &self.context)
            .field("funcs", &self.funcs.len())
            .finish()
    }
}
