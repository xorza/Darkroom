use std::str::FromStr;

use hashbrown::hash_map::{Entry, Values};
use serde::{Deserialize, Serialize};

use common::id_type;

use crate::data::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum FuncBehavior {
    #[default]
    Active,
    Passive,
}



#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputInfo {
    pub name: String,
    pub is_required: bool,
    pub data_type: DataType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<StaticValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<(String, StaticValue)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    pub data_type: DataType,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventInfo {
    pub name: String,
}

id_type!(FuncId);

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct Func {
    pub id: FuncId,
    pub name: String,
    pub category: String,
    pub behavior: FuncBehavior,
    pub is_output: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<OutputInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<EventInfo>,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct FuncLib {
    funcs: hashbrown::HashMap<FuncId, Func>,
}


impl FuncBehavior {
    pub fn toggle(&mut self) {
        *self = match *self {
            FuncBehavior::Active => FuncBehavior::Passive,
            FuncBehavior::Passive => FuncBehavior::Active,
        };
    }
}

impl FuncLib {
    pub fn from_yaml_file(file_path: &str) -> anyhow::Result<Self> {
        let yaml = std::fs::read_to_string(file_path)?;
        Ok(Self::from_yaml(&yaml)?)
    }
    pub fn from_yaml(yaml: &str) -> anyhow::Result<Self> {
        Ok(serde_yaml::from_str(&yaml)?)
    }
    pub fn to_yaml(&self) -> String {
        serde_yaml::to_string(self).unwrap()
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

impl From<&str> for EventInfo {
    fn from(s: &str) -> Self {
        EventInfo {
            name: s.to_string(),
        }
    }
}

impl<It> From<It> for FuncLib
    where
        It: IntoIterator<Item=Func>,
{
    fn from(iter: It) -> Self {
        let mut func_lib = FuncLib::default();
        for func in iter {
            func_lib.add(func);
        }
        func_lib
    }
}

impl FromStr for EventInfo {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(EventInfo {
            name: s.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::data::DataType;
    use crate::function::{Func, FuncBehavior, FuncId, FuncLib, InputInfo, OutputInfo};

    fn create_func_lib() -> FuncLib {
        [
            Func {
                id: FuncId::from_str("432b9bf1-f478-476c-a9c9-9a6e190124fc").unwrap(),
                name: "mult".to_string(),
                category: "Debug".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                inputs: vec![
                    InputInfo {
                        name: "A".to_string(),
                        is_required: true,
                        data_type: DataType::Int,
                        default_value: None,
                        variants: vec![],
                    },
                    InputInfo {
                        name: "B".to_string(),
                        is_required: true,
                        data_type: DataType::Int,
                        default_value: None,
                        variants: vec![],
                    },
                ],
                outputs: vec![OutputInfo {
                    name: "Prod".to_string(),
                    data_type: DataType::Int,
                }],
                events: vec![],
            },
            Func {
                id: FuncId::from_str("d4d27137-5a14-437a-8bb5-b2f7be0941a2").unwrap(),
                name: "get_a".to_string(),
                category: "Debug".to_string(),
                behavior: FuncBehavior::Active,
                is_output: false,
                inputs: vec![],
                outputs: vec![OutputInfo {
                    name: "Int32 Value".to_string(),
                    data_type: DataType::Int,
                }],
                events: vec![],
            },
            Func {
                id: FuncId::from_str("a937baff-822d-48fd-9154-58751539b59b").unwrap(),
                name: "get_b".to_string(),
                category: "Debug".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                inputs: vec![],
                outputs: vec![OutputInfo {
                    name: "Int32 Value".to_string(),
                    data_type: DataType::Int,
                }],
                events: vec![],
            },
            Func {
                id: FuncId::from_str("2d3b389d-7b58-44d9-b3d1-a595765b21a5").unwrap(),
                name: "sum".to_string(),
                category: "Debug".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                inputs: vec![
                    InputInfo {
                        name: "A".to_string(),
                        is_required: true,
                        data_type: DataType::Int,
                        default_value: None,
                        variants: vec![],
                    },
                    InputInfo {
                        name: "B".to_string(),
                        is_required: true,
                        data_type: DataType::Int,
                        default_value: None,
                        variants: vec![],
                    },
                ],
                outputs: vec![OutputInfo {
                    name: "Sum".to_string(),
                    data_type: DataType::Int,
                }],
                events: vec![],
            },
            Func {
                id: FuncId::from_str("f22cd316-1cdf-4a80-b86c-1277acd1408a").unwrap(),
                name: "print".to_string(),
                category: "Debug".to_string(),
                behavior: FuncBehavior::Passive,
                is_output: false,
                inputs: vec![InputInfo {
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
    fn serialization() {
        let yaml1 = std::fs::read_to_string("../test_resources/test_funcs.yml").unwrap();
        let func_lib = create_func_lib();
        let yaml2 = serde_yaml::to_string(&func_lib).unwrap();

        assert_eq!(yaml1, yaml2);

        // std::fs::write("../test_resources/test_funcs.yml", serialized).unwrap();
    }
}
