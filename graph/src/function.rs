use std::any::Any;
use std::sync::Arc;

use crate::context::ContextType;

use crate::event::EventLambda;
use crate::lambda::FuncLambda;
use crate::{async_lambda, data::*};
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
    pub fn by_id_mut(&mut self, id: &FuncId) -> Option<&mut Func> {
        assert!(!id.is_nil());
        self.funcs.by_key_mut(id)
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

#[derive(Debug, Default)]
pub struct NodeState {
    boxed: Option<Box<dyn Any + Send>>,
}

impl NodeState {
    pub(crate) fn default() -> NodeState {
        NodeState { boxed: None }
    }

    pub fn is_none(&self) -> bool {
        self.boxed.is_none()
    }

    pub fn is_some<T>(&self) -> bool
    where
        T: Any + Send,
    {
        match &self.boxed {
            None => false,
            Some(v) => v.is::<T>(),
        }
    }

    pub fn get<T>(&self) -> Option<&T>
    where
        T: Any + Send,
    {
        self.boxed
            .as_ref()
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }

    pub fn get_mut<T>(&mut self) -> Option<&mut T>
    where
        T: Any + Send,
    {
        self.boxed
            .as_mut()
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }

    pub fn set<T>(&mut self, value: T)
    where
        T: Any + Send,
    {
        self.boxed = Some(Box::new(value));
    }

    pub fn get_or_default<T>(&mut self) -> &mut T
    where
        T: Any + Send + Default,
    {
        if self
            .boxed
            .as_mut()
            .and_then(|boxed| boxed.downcast_mut::<T>())
            .is_none()
        {
            self.boxed = Some(Box::<T>::default());
        }

        self.boxed.as_mut().unwrap().downcast_mut::<T>().unwrap()
    }

    pub fn get_or_default_with<T, F>(&mut self, f: F) -> &mut T
    where
        T: Any + Send,
        F: FnOnce() -> T,
    {
        if self
            .boxed
            .as_mut()
            .and_then(|boxed| boxed.downcast_mut::<T>())
            .is_none()
        {
            self.boxed = Some(Box::<T>::new(f()));
        }

        self.boxed.as_mut().unwrap().downcast_mut::<T>().unwrap()
    }
}

pub struct TestFuncHooks {
    pub get_a: Arc<dyn Fn() -> i64 + Send + Sync + 'static>,
    pub get_b: Arc<dyn Fn() -> i64 + Send + Sync + 'static>,
    pub print: Arc<dyn Fn(i64) + Send + Sync + 'static>,
}

impl Default for TestFuncHooks {
    fn default() -> Self {
        Self {
            get_a: Arc::new(|| panic!("Unexpected call to get_a")),
            get_b: Arc::new(|| panic!("Unexpected call to get_b")),
            print: Arc::new(|_| panic!("Unexpected call to print")),
        }
    }
}

pub fn test_func_lib(hooks: TestFuncHooks) -> FuncLib {
    let TestFuncHooks {
        get_a,
        get_b,
        print,
    } = hooks;

    [
        Func {
            id: "432b9bf1-f478-476c-a9c9-9a6e190124fc".into(),
            name: "mult".to_string(),
            description: Some("Multiplies two integer values (A * B)".to_string()),
            category: "Debug".to_string(),
            behavior: FuncBehavior::Pure,
            terminal: false,
            inputs: vec![
                FuncInput {
                    name: "A".to_string(),
                    required: true,
                    data_type: DataType::Int,
                    default_value: None,
                    value_options: vec![],
                },
                FuncInput {
                    name: "B".to_string(),
                    required: false,
                    data_type: DataType::Int,
                    default_value: None,
                    value_options: vec![],
                },
            ],
            outputs: vec![FuncOutput {
                name: "Prod".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: async_lambda!(move |_, state, inputs, _, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let a: i64 = inputs[0].value.as_i64();
                // let b: i64 = inputs[1].value.as_int();
                let b: i64 = inputs[1].value.none_or_int().unwrap_or(1);
                outputs[0] = (a * b).into();
                state.set(a * b);

                Ok(())
            }),
        },
        Func {
            id: "d4d27137-5a14-437a-8bb5-b2f7be0941a2".into(),
            name: "get_a".to_string(),
            description: Some("Returns the value from test hook A".to_string()),
            category: "Debug".to_string(),
            behavior: FuncBehavior::Pure,
            terminal: false,
            inputs: vec![],
            outputs: vec![FuncOutput {
                name: "Int32 Value".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: async_lambda!(
                move |_, _, _, _, outputs| { get_a = Arc::clone(&get_a) } => {
                    assert_eq!(outputs.len(), 1);
                    outputs[0] = (get_a() as f64).into();
                    Ok(())
                }
            ),
        },
        Func {
            id: "a937baff-822d-48fd-9154-58751539b59b".into(),
            name: "get_b".to_string(),
            description: Some("Returns the value from test hook B (impure)".to_string()),
            category: "Debug".to_string(),
            behavior: FuncBehavior::Impure,
            terminal: false,
            inputs: vec![],
            outputs: vec![FuncOutput {
                name: "Int32 Value".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: async_lambda!(
                move |_, _, _, _, outputs| { get_b = Arc::clone(&get_b) } => {
                    assert_eq!(outputs.len(), 1);
                    outputs[0] = (get_b() as f64).into();
                    Ok(())
                }
            ),
        },
        Func {
            id: "2d3b389d-7b58-44d9-b3d1-a595765b21a5".into(),
            name: "sum".to_string(),
            description: Some("Adds two integer values (A + B)".to_string()),
            category: "Debug".to_string(),
            behavior: FuncBehavior::Pure,
            terminal: false,
            inputs: vec![
                FuncInput {
                    name: "A".to_string(),
                    required: true,
                    data_type: DataType::Int,
                    default_value: None,
                    value_options: vec![],
                },
                FuncInput {
                    name: "B".to_string(),
                    required: false,
                    data_type: DataType::Int,
                    default_value: None,
                    value_options: vec![],
                },
            ],
            outputs: vec![FuncOutput {
                name: "Sum".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: async_lambda!(move |_, state, inputs, _, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);
                let a: i64 = inputs[0].value.as_i64();
                let b: i64 = inputs[1].value.none_or_int().unwrap_or_default();
                // let b: i64 = inputs[1].value.as_int();
                state.set(a + b);
                outputs[0] = (a + b).into();
                Ok(())
            }),
        },
        Func {
            id: "f22cd316-1cdf-4a80-b86c-1277acd1408a".into(),
            name: "print".to_string(),
            description: Some("Outputs an integer value via the test print hook".to_string()),
            category: "Debug".to_string(),
            behavior: FuncBehavior::Impure,
            terminal: true,
            inputs: vec![FuncInput {
                name: "message".to_string(),
                required: true,
                data_type: DataType::Int,
                default_value: None,
                value_options: vec![],
            }],
            outputs: vec![],
            events: vec![],
            required_contexts: vec![],
            lambda: async_lambda!(
                move |_, _, inputs, _, _| { print = Arc::clone(&print) } => {
                    // tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    assert_eq!(inputs.len(), 1);
                    print(inputs[0].value.as_i64());
                    Ok(())
                }
            ),
        },
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use crate::context::ContextManager;
    use crate::data::DynamicValue;
    use crate::execution_graph::OutputUsage;
    use crate::function::{NodeState, TestFuncHooks, test_func_lib};
    use crate::lambda::InvokeInput;
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
        let mut node_state = NodeState::default();
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
        let event_states: Vec<common::Shared<NodeState>> = vec![];
        func_lib
            .by_id(&sum_id)
            .unwrap()
            .lambda
            .invoke(
                &mut ctx_manager,
                &mut node_state,
                &inputs,
                &outputs_meta,
                &mut outputs,
                &event_states,
            )
            .await?;
        assert_eq!(outputs[0].as_i64(), 6);
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
                &inputs,
                &outputs_meta,
                &mut outputs,
                &event_states,
            )
            .await?;
        assert_eq!(outputs[0].as_i64(), 8);

        Ok(())
    }
}
