use std::collections::HashMap;

use imaginarium::wgpu::wgpu_context::WgpuContext;

use crate::data::{DataType, DynamicValue};
use crate::elements::image::IMAGE_DATA_TYPE;
use crate::function::{Function, FunctionId};
use crate::invoke::{InvokeArgs, Invoker, TypeConverterDesc};
use crate::runtime_graph::InvokeContext;
use crate::wgpu::texture::TEXTURE_DATA_TYPE;

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

    fn all_converters(&self) -> Vec<TypeConverterDesc> {
        vec![
            TypeConverterDesc {
                src: IMAGE_DATA_TYPE.clone(),
                dst: TEXTURE_DATA_TYPE.clone(),
            },
            TypeConverterDesc {
                src: TEXTURE_DATA_TYPE.clone(),
                dst: IMAGE_DATA_TYPE.clone(),
            },
        ]
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

    fn convert_value(&self, _src_value: &DynamicValue, _dst_data_type: &DataType) -> DynamicValue {
        unimplemented!("TODO: Implement Invoker::convert_value in {}", std::any::type_name::<Self>());
    }
}

