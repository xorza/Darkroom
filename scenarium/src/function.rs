use crate::context::ContextType;

use crate::data::*;
use crate::event_lambda::EventLambda;
use crate::func_lambda::FuncLambda;
use crate::graph::NodeBehavior;
use common::id_type;
use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use common::{SerdeFormat, deserialize, serialize};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum FuncBehavior {
    // could return different values for same inputs
    #[default]
    Impure,
    // always returns the same value for same inputs
    Pure,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValueOption {
    pub name: String,
    pub value: StaticValue,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FuncInput {
    pub name: String,
    pub required: bool,
    pub data_type: DataType,
    #[serde(default)]
    pub default_value: Option<StaticValue>,
    #[serde(default)]
    pub value_options: Vec<ValueOption>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FuncOutput {
    pub name: String,
    pub data_type: DataType,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FuncEvent {
    pub name: String,

    #[serde(skip, default)]
    pub event_lambda: EventLambda,
}

id_type!(FuncId);

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Func {
    pub id: FuncId,
    pub name: String,
    pub category: String,
    pub terminal: bool,

    pub behavior: FuncBehavior,
    pub node_default_behavior: NodeBehavior,

    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub inputs: Vec<FuncInput>,
    #[serde(default)]
    pub outputs: Vec<FuncOutput>,
    #[serde(default)]
    pub events: Vec<FuncEvent>,
    #[serde(skip, default)]
    pub required_contexts: Vec<ContextType>,

    #[serde(skip, default)]
    pub lambda: FuncLambda,
}

impl KeyIndexKey<FuncId> for Func {
    fn key(&self) -> &FuncId {
        &self.id
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct FuncLib {
    pub funcs: KeyIndexVec<FuncId, Func>,
}

impl Func {
    fn validate(&self) {
        assert!(
            !self.outputs.is_empty() || self.behavior == FuncBehavior::Impure,
            "Function with no outputs should be impure"
        );
    }
}

impl FuncLib {
    pub fn deserialize(serialized: &[u8], format: SerdeFormat) -> anyhow::Result<Self> {
        deserialize(serialized, format)
    }
    pub fn serialize(&self, format: SerdeFormat) -> Vec<u8> {
        serialize(&self, format)
    }

    pub fn by_id(&self, id: &FuncId) -> Option<&Func> {
        assert!(!id.is_nil());
        self.funcs.by_key(id)
    }
    pub fn by_name(&self, name: &str) -> Option<&Func> {
        assert!(!name.is_empty());
        self.funcs.iter().find(|func| func.name == name)
    }
    pub fn by_name_mut(&mut self, name: &str) -> Option<&mut Func> {
        assert!(!name.is_empty());
        self.funcs.iter_mut().find(|func| func.name == name)
    }
    pub fn add(&mut self, func: Func) {
        func.validate();

        self.funcs.add(func);
    }

    pub fn merge<T: Into<FuncLib>>(&mut self, other: T) {
        let other = other.into();
        for func in other.funcs.items {
            self.add(func);
        }
    }
}

impl<It> From<It> for FuncLib
where
    It: IntoIterator<Item = Func>,
{
    fn from(iter: It) -> Self {
        let mut func_lib = FuncLib::default();
        for func in iter {
            func_lib.add(func);
        }
        func_lib
    }
}

#[cfg(test)]
mod tests {
    use crate::context::ContextManager;
    use crate::data::DynamicValue;
    use crate::execution_graph::OutputUsage;
    use crate::func_lambda::InvokeInput;
    use crate::prelude::AnyState;
    use crate::testing::{TestFuncHooks, test_func_lib};
    use common::SerdeFormat;

    #[test]
    fn roundtrip_serialization() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks::default());

        for format in SerdeFormat::all_formats_for_testing() {
            let serialized = func_lib.serialize(format);
            let deserialized = super::FuncLib::deserialize(&serialized, format)?;
            let serialized_again = deserialized.serialize(format);
            assert_eq!(serialized, serialized_again);
        }

        Ok(())
    }

    #[tokio::test]
    async fn invoke_by_id_and_index() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks::default());
        let sum_id = func_lib.by_name("sum").unwrap().id;

        let mut ctx_manager = ContextManager::default();
        let mut node_state = AnyState::default();
        let mut inputs = vec![
            InvokeInput {
                changed: true,
                value: DynamicValue::Int(2),
            },
            InvokeInput {
                changed: true,
                value: DynamicValue::Int(4),
            },
        ];
        let mut outputs = vec![DynamicValue::None];
        let outputs_meta = vec![OutputUsage::Needed; outputs.len()];
        let event_state = crate::common::shared_any_state::SharedAnyState::default();
        func_lib
            .by_id(&sum_id)
            .unwrap()
            .lambda
            .invoke(
                &mut ctx_manager,
                &mut node_state,
                &event_state,
                &inputs,
                &outputs_meta,
                &mut outputs,
            )
            .await?;
        assert_eq!(outputs[0].as_i64().unwrap(), 6);
        let cached = *node_state
            .get::<i64>()
            .expect("InvokeCache should contain the sum value");
        assert_eq!(cached, 6);

        inputs[0].value = DynamicValue::Int(3);
        inputs[1].value = DynamicValue::Int(5);
        outputs[0] = DynamicValue::None;
        func_lib
            .by_id(&sum_id)
            .unwrap()
            .lambda
            .invoke(
                &mut ctx_manager,
                &mut node_state,
                &event_state,
                &inputs,
                &outputs_meta,
                &mut outputs,
            )
            .await?;
        assert_eq!(outputs[0].as_i64().unwrap(), 8);

        Ok(())
    }
}
