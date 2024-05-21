use std::str::FromStr;

use hashbrown::hash_map::Entry;
use serde::{Deserialize, Serialize};

use common::id_type;

use crate::data::*;
use crate::graph::FuncBehavior;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    pub data_type: DataType,
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

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct FuncLib {
    funcs: hashbrown::HashMap<FuncId, Func>,
}

impl FuncLib {
    pub fn get_func_by_id(&self, id: FuncId) -> Option<&Func> {
        self.funcs.get(&id)
    }
    pub fn get_func_by_id_mut(&mut self, id: FuncId) -> Option<&mut Func> {
        self.funcs.get_mut(&id)
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
    pub fn iter(&self) -> hashbrown::hash_map::Iter<FuncId, Func> {
        self.funcs.iter()
    }
    pub fn merge(&mut self, other: FuncLib) {
        for (_id, func) in other.funcs {
            self.add(func);
        }
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
impl FromStr for EventInfo {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(EventInfo {
            name: s.to_string(),
        })
    }
}
