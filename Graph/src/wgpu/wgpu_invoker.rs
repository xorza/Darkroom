use std::str::FromStr;

use once_cell::sync::Lazy;

use imaginarium::wgpu::wgpu_context::WgpuContext;

use crate::data::{DataType, TypeId};
use crate::function::{Function, FunctionId};
use crate::invoke::{InvokeArgs, Invoker};
use crate::runtime_graph::InvokeContext;

pub static IMAGE_DATA_TYPE: Lazy<DataType> = Lazy::new(||
    DataType::Custom {
        type_id: TypeId::from_str("9b21b096-caa3-4443-ad43-bf425fcc975e").unwrap(),
        type_name: "Image".to_string(),
    }
);

struct WgpuFunc {
    shader: imaginarium::wgpu::wgpu_context::Shader,
    function: Function,
}

pub(crate) struct WgpuInvoker {
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

impl WgpuFunc {}


