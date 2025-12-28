use std::str::FromStr;

use crate::common::FileFormat;
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
    // function designed to be terminal in graph, i.e. save results to io
    Output,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValueVariant {
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

    pub behavior: FuncBehavior,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<FuncInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<FuncOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<FuncEvent>,
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
        let funcs: Vec<Func> = match format {
            FileFormat::Yaml => serde_yml::from_str(serialized)?,
            FileFormat::Json => serde_json::from_str(serialized)?,
        };

        Ok(funcs.into())
    }
    pub fn serialize(&self, format: FileFormat) -> String {
        let mut funcs = self.funcs.clone();
        funcs.sort_by_key(|func| func.id);

        match format {
            FileFormat::Yaml => serde_yml::to_string(&funcs)
                .expect("Failed to serialize function library to YAML")
                .normalize(),
            FileFormat::Json => serde_json::to_string_pretty(&funcs)
                .expect("Failed to serialize function library to JSON")
                .normalize(),
        }
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

pub fn test_func_lib() -> FuncLib {
    [
        Func {
            id: FuncId::from_str("432b9bf1-f478-476c-a9c9-9a6e190124fc")
                .expect("Failed to parse FuncId for mult"),
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
                    variants: vec![],
                },
                FuncInput {
                    name: "B".to_string(),
                    required: true,
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
            id: FuncId::from_str("d4d27137-5a14-437a-8bb5-b2f7be0941a2")
                .expect("Failed to parse FuncId for get_a"),
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
        },
        Func {
            id: FuncId::from_str("a937baff-822d-48fd-9154-58751539b59b")
                .expect("Failed to parse FuncId for get_b"),
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
        },
        Func {
            id: FuncId::from_str("2d3b389d-7b58-44d9-b3d1-a595765b21a5")
                .expect("Failed to parse FuncId for sum"),
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
                    variants: vec![],
                },
                FuncInput {
                    name: "B".to_string(),
                    required: true,
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
            id: FuncId::from_str("f22cd316-1cdf-4a80-b86c-1277acd1408a")
                .expect("Failed to parse FuncId for print"),
            name: "print".to_string(),
            description: None,
            category: "Debug".to_string(),
            behavior: FuncBehavior::Impure,
            inputs: vec![FuncInput {
                name: "message".to_string(),
                required: true,
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

#[cfg(test)]
const TEST_FUNCS_YAML: &str = r#"- id: "2d3b389d-7b58-44d9-b3d1-a595765b21a5"
  name: sum
  category: Debug
  behavior: Pure
  inputs:
    - name: A
      required: true
      data_type: Int
    - name: B
      required: true
      data_type: Int
  outputs:
    - name: Sum
      data_type: Int
- id: "432b9bf1-f478-476c-a9c9-9a6e190124fc"
  name: mult
  category: Debug
  behavior: Pure
  inputs:
    - name: A
      required: true
      data_type: Int
    - name: B
      required: true
      data_type: Int
  outputs:
    - name: Prod
      data_type: Int
- id: a937baff-822d-48fd-9154-58751539b59b
  name: get_b
  category: Debug
  behavior: Pure
  outputs:
    - name: Int32 Value
      data_type: Int
- id: d4d27137-5a14-437a-8bb5-b2f7be0941a2
  name: get_a
  category: Debug
  behavior: Impure
  outputs:
    - name: Int32 Value
      data_type: Int
- id: f22cd316-1cdf-4a80-b86c-1277acd1408a
  name: print
  category: Debug
  behavior: Impure
  inputs:
    - name: message
      required: true
      data_type: Int
"#;

#[cfg(test)]
mod tests {
    use crate::common::FileFormat;
    use crate::function::test_func_lib;
    use common::yaml_format::reformat_yaml;

    #[test]
    fn serialization() {
        let func_lib = test_func_lib();
        let file_yaml = reformat_yaml(super::TEST_FUNCS_YAML)
            .expect("Failed to normalize embedded test function YAML");
        let serialized_yaml = func_lib.serialize(FileFormat::Yaml);

        assert_eq!(file_yaml, serialized_yaml);
    }

    #[test]
    fn roundtrip_serialization() -> anyhow::Result<()> {
        let func_lib = test_func_lib();

        for format in [FileFormat::Yaml, FileFormat::Json] {
            let serialized = func_lib.serialize(format);
            let deserialized = super::FuncLib::deserialize(serialized.as_str(), format)?;
            let serialized_again = deserialized.serialize(format);
            assert_eq!(serialized_again, serialized);
        }

        Ok(())
    }
}
