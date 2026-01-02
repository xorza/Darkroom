use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

use crate::context::{ContextManager, ContextType};
use crate::execution_graph::OutputUsage;
use crate::prelude::InputState;
use crate::{async_lambda, data::*};
use common::id_type;
use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use common::{deserialize, serialize, FileFormat};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum FuncBehavior {
    // could return different values for same inputs
    #[default]
    Impure,
    // always returns the same value for same inputs
    Pure,
}

#[derive(Debug, Error)]
pub enum InvokeError {
    #[error("Invocation failed: {0}")]
    External(#[from] anyhow::Error),
}

pub type InvokeResult<T> = Result<T, InvokeError>;

#[derive(Debug)]
pub struct InvokeInput {
    pub state: InputState,
    pub value: DynamicValue,
}

type AsyncLambdaFuture<'a> = Pin<Box<dyn Future<Output = InvokeResult<()>> + Send + 'a>>;

pub trait AsyncLambdaFn:
    for<'a> Fn(
        &'a mut ContextManager,
        &'a mut InvokeCache,
        &'a [InvokeInput],
        &'a [OutputUsage],
        &'a mut [DynamicValue],
    ) -> AsyncLambdaFuture<'a>
    + Send
    + Sync
    + 'static
{
}

impl<T> AsyncLambdaFn for T where
    T: for<'a> Fn(
            &'a mut ContextManager,
            &'a mut InvokeCache,
            &'a [InvokeInput],
            &'a [OutputUsage],
            &'a mut [DynamicValue],
        ) -> AsyncLambdaFuture<'a>
        + Send
        + Sync
        + 'static
{
}

pub type AsyncLambda = dyn AsyncLambdaFn;

#[derive(Clone, Default)]
pub enum FuncLambda {
    #[default]
    None,
    Lambda(Arc<AsyncLambda>),
}

impl FuncLambda {
    pub fn new<F>(lambda: F) -> Self
    where
        F: AsyncLambdaFn,
    {
        Self::Lambda(Arc::new(lambda))
    }

    pub async fn invoke(
        &self,
        ctx_manager: &mut ContextManager,
        cache: &mut InvokeCache,
        inputs: &[InvokeInput],
        output_usage: &[OutputUsage],
        outputs: &mut [DynamicValue],
    ) -> InvokeResult<()> {
        match self {
            FuncLambda::None => {
                panic!("Func missing lambda");
            }
            FuncLambda::Lambda(inner) => {
                (inner)(ctx_manager, cache, inputs, output_usage, outputs).await
            }
        }
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<StaticValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
}

id_type!(FuncId);

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Func {
    pub id: FuncId,
    pub name: String,
    pub category: String,

    pub behavior: FuncBehavior,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<FuncInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<FuncOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
    pub fn terminal(&self) -> bool {
        self.validate().unwrap();
        self.outputs.is_empty()
    }

    fn validate(&self) -> anyhow::Result<()> {
        assert!(
            !self.outputs.is_empty() || self.behavior == FuncBehavior::Impure,
            "Function with no outputs should be impure"
        );
        Ok(())
    }
}

impl FuncLib {
    pub fn deserialize(serialized: &str, format: FileFormat) -> anyhow::Result<Self> {
        Ok(deserialize(serialized, format)?)
    }
    pub fn serialize(&self, format: FileFormat) -> String {
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
        func.validate().unwrap();

        self.funcs.push(func);
    }

    pub fn merge(&mut self, other: FuncLib) {
        for func in other.funcs.items {
            self.add(func);
        }
    }
}

impl From<&str> for FuncEvent {
    fn from(s: &str) -> Self {
        FuncEvent {
            name: s.to_string(),
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

impl FromStr for FuncEvent {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(FuncEvent {
            name: s.to_string(),
        })
    }
}

impl std::fmt::Debug for FuncLambda {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FuncLambda::None => f.debug_struct("FuncLambda::None").finish(),
            FuncLambda::Lambda(_) => f.debug_struct("FuncLambda::Lambda").finish(),
        }
    }
}

#[derive(Debug, Default)]
pub struct InvokeCache {
    boxed: Option<Box<dyn Any + Send>>,
}

impl InvokeCache {
    pub(crate) fn default() -> InvokeCache {
        InvokeCache { boxed: None }
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
            description: None,
            category: "Debug".to_string(),
            behavior: FuncBehavior::Pure,
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
                    required: true,
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
            lambda: async_lambda!(move |_ctx, cache, inputs, _output_usage, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let a: i64 = inputs[0].value.as_int();
                let b: i64 = inputs[1].value.as_int();
                outputs[0] = (a * b).into();
                cache.set(a * b);

                Ok(())
            }),
        },
        Func {
            id: "d4d27137-5a14-437a-8bb5-b2f7be0941a2".into(),
            name: "get_a".to_string(),
            description: None,
            category: "Debug".to_string(),
            behavior: FuncBehavior::Pure,
            inputs: vec![],
            outputs: vec![FuncOutput {
                name: "Int32 Value".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: async_lambda!(
                move |_ctx, _cache, _inputs, _output_usage, outputs| { get_a = Arc::clone(&get_a) } => {
                    assert_eq!(outputs.len(), 1);
                    outputs[0] = (get_a() as f64).into();
                    Ok(())
                }
            ),
        },
        Func {
            id: "a937baff-822d-48fd-9154-58751539b59b".into(),
            name: "get_b".to_string(),
            description: None,
            category: "Debug".to_string(),
            behavior: FuncBehavior::Impure,
            inputs: vec![],
            outputs: vec![FuncOutput {
                name: "Int32 Value".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: async_lambda!(
                move |_ctx, _cache, _inputs, _output_usage, outputs| { get_b = Arc::clone(&get_b) } => {
                    assert_eq!(outputs.len(), 1);
                    outputs[0] = (get_b() as f64).into();
                    Ok(())
                }
            ),
        },
        Func {
            id: "2d3b389d-7b58-44d9-b3d1-a595765b21a5".into(),
            name: "sum".to_string(),
            description: None,
            category: "Debug".to_string(),
            behavior: FuncBehavior::Pure,
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
                    required: true,
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
            lambda: async_lambda!(move |_ctx_manager, cache, inputs, _output_usage, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);
                let a: i64 = inputs[0].value.as_int();
                let b: i64 = inputs[1].value.as_int();
                cache.set(a + b);
                outputs[0] = (a + b).into();
                Ok(())
            }),
        },
        Func {
            id: "f22cd316-1cdf-4a80-b86c-1277acd1408a".into(),
            name: "print".to_string(),
            description: None,
            category: "Debug".to_string(),
            behavior: FuncBehavior::Impure,
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
                move |_ctx_manager, _cache, inputs, _output_usage, _outputs| { print = Arc::clone(&print) } => {
                    // tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    assert_eq!(inputs.len(), 1);
                    print(inputs[0].value.as_int());
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
    use crate::execution_graph::{InputState, OutputUsage};
    use crate::function::{test_func_lib, InvokeCache, InvokeInput, TestFuncHooks};
    use common::FileFormat;

    #[test]
    fn roundtrip_serialization() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks::default());

        for format in [FileFormat::Yaml, FileFormat::Json, FileFormat::Lua] {
            let serialized = func_lib.serialize(format);
            let deserialized = super::FuncLib::deserialize(serialized.as_str(), format)?;
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
        let mut cache = InvokeCache::default();
        let mut inputs = vec![
            InvokeInput {
                state: InputState::Changed,
                value: DynamicValue::Int(2),
            },
            InvokeInput {
                state: InputState::Changed,
                value: DynamicValue::Int(4),
            },
        ];
        let mut outputs = vec![DynamicValue::None];
        let outputs_meta = vec![OutputUsage::Needed; outputs.len()];
        func_lib
            .by_id(&sum_id)
            .unwrap()
            .lambda
            .invoke(
                &mut ctx_manager,
                &mut cache,
                &inputs,
                &outputs_meta,
                &mut outputs,
            )
            .await?;
        assert_eq!(outputs[0].as_int(), 6);
        let cached = *cache
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
                &mut cache,
                &inputs,
                &outputs_meta,
                &mut outputs,
            )
            .await?;
        assert_eq!(outputs[0].as_int(), 8);

        Ok(())
    }
}
