use std::any::Any;
use std::str::FromStr;
use std::sync::Arc;

use crate::data::*;
use common::id_type;
use common::{deserialize, serialize, FileFormat};
use hashbrown::hash_map::{Entry, Values};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum FuncBehavior {
    // could return different values for same inputs
    #[default]
    Impure,
    // always returns the same value for same inputs
    Pure,
    // function designed to be terminal in graph, i.e. save results to io
    Output,
}

pub type InvokeArgs = [DynamicValue];

#[derive(Debug, Error)]
pub enum InvokeError {
    #[error("Invocation failed: {0}")]
    External(#[from] anyhow::Error),
}

pub type InvokeResult<T> = Result<T, InvokeError>;

pub type Lambda = dyn Fn(&mut InvokeCache, &InvokeArgs, &mut InvokeArgs) -> InvokeResult<()>
    + Send
    + Sync
    + 'static;

#[derive(Clone, Default)]
pub enum FuncLambda {
    #[default]
    None,
    Lambda(Arc<Lambda>),
}

impl FuncLambda {
    pub fn new<F>(lambda: F) -> Self
    where
        F: Fn(&mut InvokeCache, &InvokeArgs, &mut InvokeArgs) -> InvokeResult<()>
            + Send
            + Sync
            + 'static,
    {
        Self::Lambda(Arc::new(lambda))
    }

    pub fn invoke(
        &self,
        cache: &mut InvokeCache,
        inputs: &InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> InvokeResult<()> {
        match self {
            FuncLambda::None => {
                panic!("Func missing lambda");
            }
            FuncLambda::Lambda(inner) => (inner)(cache, inputs, outputs),
        }
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
        let is_some = self.is_some::<T>();

        if is_some {
            self.boxed
                .as_mut()
                .expect("InvokeCache missing value")
                .downcast_mut::<T>()
                .expect("InvokeCache has unexpected type")
        } else {
            self.boxed
                .insert(Box::<T>::default())
                .downcast_mut::<T>()
                .expect("InvokeCache default insert failed")
        }
    }

    pub fn get_or_default_with<T, F>(&mut self, f: F) -> &mut T
    where
        T: Any + Send,
        F: FnOnce() -> T,
    {
        let is_some = self.is_some::<T>();

        if is_some {
            self.boxed
                .as_mut()
                .expect("InvokeCache missing value")
                .downcast_mut::<T>()
                .expect("InvokeCache has unexpected type")
        } else {
            self.boxed
                .insert(Box::<T>::new(f()))
                .downcast_mut::<T>()
                .expect("InvokeCache insert failed")
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
    pub lambda: FuncLambda,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct FuncLib {
    pub funcs: Vec<Func>,
}

impl FuncLib {
    pub fn from_file(file_path: &str) -> anyhow::Result<Self> {
        let format = FileFormat::from_file_name(file_path)
            .expect("Failed to infer function library file format from file name");
        let contents = std::fs::read_to_string(file_path)?;
        Self::deserialize(&contents, format)
    }
    pub fn deserialize(serialized: &str, format: FileFormat) -> anyhow::Result<Self> {
        let funcs: Vec<Func> = deserialize(serialized, format)?;

        Ok(funcs.into())
    }
    pub fn serialize(&self, format: FileFormat) -> String {
        serialize(&self.funcs, format)
    }

    pub fn by_id(&self, id: FuncId) -> Option<&Func> {
        self.funcs.iter().find(|func| func.id == id)
    }
    pub fn by_id_mut(&mut self, id: FuncId) -> Option<&mut Func> {
        self.funcs.iter_mut().find(|func| func.id == id)
    }
    pub fn by_name(&self, name: &str) -> Option<&Func> {
        self.funcs.iter().find(|func| func.name == name)
    }
    pub fn by_name_mut(&mut self, name: &str) -> Option<&mut Func> {
        self.funcs.iter_mut().find(|func| func.name == name)
    }
    pub fn add(&mut self, func: Func) {
        let entry = self.by_id(func.id);
        match entry {
            Some(_) => {
                panic!("Func already exists");
            }
            None => {
                self.funcs.push(func);
            }
        }
    }

    pub fn invoke_by_id(
        &self,
        func_id: FuncId,
        cache: &mut InvokeCache,
        inputs: &InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> InvokeResult<()> {
        let func = self
            .by_id(func_id)
            .unwrap_or_else(|| panic!("Func with id {:?} not found", func_id));
        func.lambda.invoke(cache, inputs, outputs)
    }

    pub fn invoke_by_index(
        &self,
        func_idx: usize,
        cache: &mut InvokeCache,
        inputs: &InvokeArgs,
        outputs: &mut InvokeArgs,
    ) -> InvokeResult<()> {
        let func = self
            .funcs
            .get(func_idx)
            .unwrap_or_else(|| panic!("Func index {} out of bounds", func_idx));
        func.lambda.invoke(cache, inputs, outputs)
    }
    pub fn merge(&mut self, other: FuncLib) {
        for func in other.funcs {
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

pub struct TestFuncHooks {
    pub get_a: Box<dyn Fn() -> i64 + Send + Sync + 'static>,
    pub get_b: Box<dyn Fn() -> i64 + Send + Sync + 'static>,
    pub print: Box<dyn Fn(i64) + Send + Sync + 'static>,
}

impl Default for TestFuncHooks {
    fn default() -> Self {
        Self {
            get_a: Box::new(|| panic!("Unexpected call to get_a")),
            get_b: Box::new(|| panic!("Unexpected call to get_b")),
            print: Box::new(|_| panic!("Unexpected call to print")),
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
            id: FuncId::from_str("432b9bf1-f478-476c-a9c9-9a6e190124fc").unwrap(),
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
            lambda: FuncLambda::new(move |ctx, inputs, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);

                let a: i64 = inputs[0].as_int();
                let b: i64 = inputs[1].as_int();
                outputs[0] = (a * b).into();
                ctx.set(a * b);

                Ok(())
            }),
        },
        Func {
            id: FuncId::from_str("d4d27137-5a14-437a-8bb5-b2f7be0941a2").unwrap(),
            name: "get_a".to_string(),
            description: None,
            category: "Debug".to_string(),
            behavior: FuncBehavior::Impure,
            inputs: vec![],
            outputs: vec![FuncOutput {
                name: "Int32 Value".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
            lambda: FuncLambda::new(move |_, _, outputs| {
                assert_eq!(outputs.len(), 1);
                outputs[0] = (get_a() as f64).into();
                Ok(())
            }),
        },
        Func {
            id: FuncId::from_str("a937baff-822d-48fd-9154-58751539b59b").unwrap(),
            name: "get_b".to_string(),
            description: None,
            category: "Debug".to_string(),
            behavior: FuncBehavior::Pure,
            inputs: vec![],
            outputs: vec![FuncOutput {
                name: "Int32 Value".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
            lambda: FuncLambda::new(move |_, _, outputs| {
                assert_eq!(outputs.len(), 1);
                outputs[0] = (get_b() as f64).into();
                Ok(())
            }),
        },
        Func {
            id: FuncId::from_str("2d3b389d-7b58-44d9-b3d1-a595765b21a5").unwrap(),
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
            lambda: FuncLambda::new(move |ctx, inputs, outputs| {
                assert_eq!(inputs.len(), 2);
                assert_eq!(outputs.len(), 1);
                let a: i64 = inputs[0].as_int();
                let b: i64 = inputs[1].as_int();
                ctx.set(a + b);
                outputs[0] = (a + b).into();
                Ok(())
            }),
        },
        Func {
            id: FuncId::from_str("f22cd316-1cdf-4a80-b86c-1277acd1408a").unwrap(),
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
            lambda: FuncLambda::new(move |_, inputs, _| {
                assert_eq!(inputs.len(), 1);
                print(inputs[0].as_int());
                Ok(())
            }),
        },
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use crate::data::DynamicValue;
    use crate::function::{test_func_lib, InvokeCache, TestFuncHooks};
    use common::yaml_format::reformat_yaml;
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

    #[test]
    fn invoke_by_id_and_index() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks::default());
        let sum_id = func_lib.by_name("sum").unwrap().id;

        let mut cache = InvokeCache::default();
        let mut inputs = vec![DynamicValue::Int(2), DynamicValue::Int(4)];
        let mut outputs = vec![DynamicValue::None];
        func_lib.invoke_by_id(sum_id, &mut cache, &inputs, &mut outputs)?;
        assert_eq!(outputs[0].as_int(), 6);
        let cached = *cache
            .get::<i64>()
            .expect("InvokeCache should contain the sum value");
        assert_eq!(cached, 6);

        inputs[0] = DynamicValue::Int(3);
        inputs[1] = DynamicValue::Int(5);
        outputs[0] = DynamicValue::None;
        func_lib.invoke_by_id(sum_id, &mut cache, &inputs, &mut outputs)?;
        assert_eq!(outputs[0].as_int(), 8);

        Ok(())
    }
}
