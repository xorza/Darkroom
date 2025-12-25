use std::str::FromStr;

use crate::data::*;
use common::id_type;
use common::normalize_string::NormalizeString;
use hashbrown::hash_map::{Entry, Values};
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
pub struct ValueVariant {
    pub name: String,
    pub value: StaticValue,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FuncInput {
    pub name: String,
    pub is_required: bool,
    pub data_type: DataType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<StaticValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<ValueVariant>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub behavior: FuncBehavior,
    pub terminal: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<FuncInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<FuncOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<FuncEvent>,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct FuncLib {
    funcs: hashbrown::HashMap<FuncId, Func>,
}

impl FuncBehavior {
    pub fn toggle(&mut self) {
        *self = match *self {
            FuncBehavior::Impure => FuncBehavior::Pure,
            FuncBehavior::Pure => FuncBehavior::Impure,
        };
    }
}

impl FuncLib {
    pub fn from_yaml_file(file_path: &str) -> anyhow::Result<Self> {
        let yaml = std::fs::read_to_string(file_path)?;
        Self::from_yaml(&yaml)
    }
    pub fn from_yaml(yaml: &str) -> anyhow::Result<Self> {
        let funcs: Vec<Func> = serde_yml::from_str(yaml)?;

        Ok(funcs.into())
    }
    pub fn to_yaml(&self) -> String {
        let mut funcs: Vec<&Func> = self.funcs.values().collect();
        funcs.sort_by(|a, b| a.id.cmp(&b.id));

        serde_yml::to_string(&funcs)
            .expect("Failed to serialize function library to YAML")
            .normalize()
    }

    pub fn func_by_id(&self, id: FuncId) -> Option<&Func> {
        self.funcs.get(&id)
    }
    pub fn func_by_id_mut(&mut self, id: FuncId) -> Option<&mut Func> {
        self.funcs.get_mut(&id)
    }
    pub fn func_by_name(&self, name: &str) -> Option<&Func> {
        self.funcs.values().find(|func| func.name == name)
    }
    pub fn func_by_name_mut(&mut self, name: &str) -> Option<&mut Func> {
        self.funcs.values_mut().find(|func| func.name == name)
    }
    pub fn add(&mut self, func: Func) {
        let entry = self.funcs.entry(func.id);
        match entry {
            Entry::Occupied(_) => {
                panic!("Func already exists");
            }
            Entry::Vacant(_) => {
                entry.insert(func);
            }
        }
    }
    pub fn iter(&self) -> Values<'_, FuncId, Func> {
        self.funcs.values()
    }
    pub fn merge(&mut self, other: FuncLib) {
        for (_id, func) in other.funcs {
            self.add(func);
        }
    }
    pub fn len(&self) -> usize {
        self.funcs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.funcs.is_empty()
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

#[cfg(test)]
mod tests {
    use crate::data::DataType;
    use crate::function::{Func, FuncBehavior, FuncId, FuncInput, FuncLib, FuncOutput};
    use common::yaml_format::reformat_yaml;
    use std::str::FromStr;

    fn create_func_lib() -> FuncLib {
        [
            Func {
                id: FuncId::from_str("432b9bf1-f478-476c-a9c9-9a6e190124fc").unwrap(),
                name: "mult".to_string(),
                description: None,
                category: "Debug".to_string(),
                behavior: FuncBehavior::Pure,
                terminal: false,
                inputs: vec![
                    FuncInput {
                        name: "A".to_string(),
                        is_required: true,
                        data_type: DataType::Int,
                        default_value: None,
                        variants: vec![],
                    },
                    FuncInput {
                        name: "B".to_string(),
                        is_required: true,
                        data_type: DataType::Int,
                        default_value: None,
                        variants: vec![],
                    },
                ],
                outputs: vec![FuncOutput {
                    name: "Prod".to_string(),
                    data_type: DataType::Int,
                }],
                events: vec![],
            },
            Func {
                id: FuncId::from_str("d4d27137-5a14-437a-8bb5-b2f7be0941a2").unwrap(),
                name: "get_a".to_string(),
                description: None,
                category: "Debug".to_string(),
                behavior: FuncBehavior::Impure,
                terminal: false,
                inputs: vec![],
                outputs: vec![FuncOutput {
                    name: "Int32 Value".to_string(),
                    data_type: DataType::Int,
                }],
                events: vec![],
            },
            Func {
                id: FuncId::from_str("a937baff-822d-48fd-9154-58751539b59b").unwrap(),
                name: "get_b".to_string(),
                description: None,
                category: "Debug".to_string(),
                behavior: FuncBehavior::Pure,
                terminal: false,
                inputs: vec![],
                outputs: vec![FuncOutput {
                    name: "Int32 Value".to_string(),
                    data_type: DataType::Int,
                }],
                events: vec![],
            },
            Func {
                id: FuncId::from_str("2d3b389d-7b58-44d9-b3d1-a595765b21a5").unwrap(),
                name: "sum".to_string(),
                description: None,
                category: "Debug".to_string(),
                behavior: FuncBehavior::Pure,
                terminal: false,
                inputs: vec![
                    FuncInput {
                        name: "A".to_string(),
                        is_required: true,
                        data_type: DataType::Int,
                        default_value: None,
                        variants: vec![],
                    },
                    FuncInput {
                        name: "B".to_string(),
                        is_required: true,
                        data_type: DataType::Int,
                        default_value: None,
                        variants: vec![],
                    },
                ],
                outputs: vec![FuncOutput {
                    name: "Sum".to_string(),
                    data_type: DataType::Int,
                }],
                events: vec![],
            },
            Func {
                id: FuncId::from_str("f22cd316-1cdf-4a80-b86c-1277acd1408a").unwrap(),
                name: "print".to_string(),
                description: None,
                category: "Debug".to_string(),
                behavior: FuncBehavior::Pure,
                terminal: false,
                inputs: vec![FuncInput {
                    name: "message".to_string(),
                    is_required: true,
                    data_type: DataType::Int,
                    default_value: None,
                    variants: vec![],
                }],
                outputs: vec![],
                events: vec![],
            },
        ]
        .into()
    }

    #[test]
    fn serialization() -> anyhow::Result<()> {
        let file_yaml: String = {
            // This trick is used to make yaml formatting consistent
            let str = std::fs::read_to_string("../test_resources/test_funcs.yml")?;
            reformat_yaml(str.as_str())?
        };

        let func_lib = create_func_lib();
        let serialized_yaml = func_lib.to_yaml();

        assert_eq!(file_yaml, serialized_yaml);

        Ok(())
    }
}
