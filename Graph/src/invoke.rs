use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::data::{DataType, DynamicValue};
use crate::function::{Function, FunctionId};
use crate::runtime_graph::InvokeContext;

trait TypeConverterKey {
    // -> (src, dst)
    fn key(&self) -> (&DataType, &DataType);
}

#[derive(Debug, Clone)]
pub struct TypeConverterDesc {
    pub src: DataType,
    pub dst: DataType,
}

pub type InvokeArgs = [Option<DynamicValue>];

pub trait Invokable {
    fn call(&self, ctx: &mut InvokeContext, inputs: &InvokeArgs, outputs: &mut InvokeArgs);
}

pub trait Invoker {
    fn all_functions(&self) -> Vec<Function> { vec![] }
    fn all_converters(&self) -> Vec<TypeConverterDesc> { vec![] }

    fn invoke(
        &self,
        function_id: FunctionId,
        ctx: &mut InvokeContext,
        inputs: &InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()>;

    fn convert_value(&self,
                     _src_value: &DynamicValue,
                     _dst_data_type: &DataType)
        -> DynamicValue {
        unimplemented!("TODO: Implement Invoker::convert_value in {}", std::any::type_name::<Self>());
    }
}

pub struct UberInvoker {
    invokers: Vec<Box<dyn Invoker>>,
    function_id_to_invoker_index: HashMap<FunctionId, usize>,
    data_type_to_invoker_index: HashMap<TypeConverterDesc, usize>,
}


impl UberInvoker {
    pub fn new(invokers: Vec<Box<dyn Invoker>>) -> Self {
        let mut function_id_to_invoker_index = HashMap::new();
        invokers
            .iter()
            .enumerate()
            .for_each(|(index, invoker)| {
                invoker
                    .all_functions()
                    .iter()
                    .for_each(|function| {
                        function_id_to_invoker_index.insert(function.self_id, index);
                    });
            });

        let mut data_type_to_invoker_index = HashMap::new();
        invokers
            .iter()
            .enumerate()
            .for_each(|(index, invoker)| {
                invoker
                    .all_converters()
                    .iter()
                    .for_each(|converter| {
                        data_type_to_invoker_index.insert(converter.clone(), index);
                    });
            });

        Self {
            invokers,
            function_id_to_invoker_index,
            data_type_to_invoker_index,
        }
    }
}

impl Invoker for UberInvoker {
    fn all_functions(&self) -> Vec<Function> {
        self.invokers.iter().flat_map(|invoker| invoker.all_functions()).collect()
    }
    fn all_converters(&self) -> Vec<TypeConverterDesc> {
        self.invokers.iter().flat_map(|invoker| invoker.all_converters()).collect()
    }

    fn invoke(
        &self,
        function_id: FunctionId,
        ctx: &mut InvokeContext,
        inputs: &InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> anyhow::Result<()>
    {
        let &invoker_index = self.function_id_to_invoker_index
            .get(&function_id)
            .expect("Missing invoker for function_id");

        let invoker = &self.invokers[invoker_index];
        invoker.invoke(function_id, ctx, inputs, outputs)
    }

    fn convert_value(&self, src_value: &DynamicValue, dst_data_type: &DataType) -> DynamicValue {
        let converter_desc = (src_value.data_type(), dst_data_type);

        let &invoker_index = self.data_type_to_invoker_index
            .get(&converter_desc as &dyn TypeConverterKey)
            .expect("Missing invoker for data_type");

        let invoker = &self.invokers[invoker_index];
        invoker.convert_value(src_value, dst_data_type)
    }
}


impl TypeConverterKey for TypeConverterDesc {
    fn key(&self) -> (&DataType, &DataType) {
        (&self.src, &self.dst)
    }
}
impl<'a> TypeConverterKey for (&'a DataType, &'a DataType) {
    fn key(&self) -> (&DataType, &DataType) {
        (self.0, self.1)
    }
}

impl Hash for dyn TypeConverterKey + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}
impl PartialEq for dyn TypeConverterKey + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}
impl Eq for dyn TypeConverterKey + '_ {}

impl Hash for TypeConverterDesc {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}
impl PartialEq for TypeConverterDesc {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}
impl Eq for TypeConverterDesc {}

impl<'a> Borrow<dyn TypeConverterKey + 'a> for TypeConverterDesc {
    fn borrow(&self) -> &(dyn TypeConverterKey + 'a) {
        self
    }
}
impl<'a> Borrow<dyn TypeConverterKey + 'a> for (&'a DataType, &'a DataType) {
    fn borrow(&self) -> &(dyn TypeConverterKey + 'a) {
        self
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_converter_desc_hash() {
        let mut map = HashMap::new();
        map.insert(
            TypeConverterDesc { src: DataType::Int, dst: DataType::Int },
            13,
        );

        let borrowed = (&DataType::Int, &DataType::Int);

        assert_eq!(map.get(&borrowed as &dyn TypeConverterKey), Some(&13));
    }
}

