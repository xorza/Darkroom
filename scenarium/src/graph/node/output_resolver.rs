use std::hash::Hash;

use hashbrown::HashMap;

use crate::{DataType, StaticValue};

#[derive(Debug)]
pub(crate) enum OutputSource<K> {
    Fixed(DataType),
    Bind(K),
    Const {
        declared: DataType,
        value: StaticValue,
    },
    Unresolved,
}

#[derive(Debug)]
enum ResolutionState {
    Resolving,
    Resolved(DataType),
}

#[derive(Debug)]
pub(crate) struct OutputResolver<K> {
    states: HashMap<K, ResolutionState>,
    path: Vec<K>,
}

impl<K> OutputResolver<K> {
    pub(crate) fn new() -> Self {
        Self {
            states: HashMap::new(),
            path: Vec::new(),
        }
    }
}

impl<K> OutputResolver<K>
where
    K: Copy + Eq + Hash,
{
    pub(crate) fn resolve(
        &mut self,
        output: K,
        source: &impl Fn(K) -> OutputSource<K>,
    ) -> DataType {
        self.path.clear();
        let mut current = output;
        let data_type = loop {
            match self.states.get(&current) {
                Some(ResolutionState::Resolving) => break DataType::Any,
                Some(ResolutionState::Resolved(data_type)) => break data_type.clone(),
                None => {}
            }
            self.states.insert(current, ResolutionState::Resolving);
            self.path.push(current);
            match source(current) {
                OutputSource::Fixed(data_type) => break data_type,
                OutputSource::Bind(bound) => current = bound,
                OutputSource::Const { declared, value } => {
                    break constant_output_type(declared, value);
                }
                OutputSource::Unresolved => break DataType::Any,
            }
        };
        for output in self.path.drain(..) {
            self.states
                .insert(output, ResolutionState::Resolved(data_type.clone()));
        }
        data_type
    }
}

fn constant_output_type(declared: DataType, value: StaticValue) -> DataType {
    if !matches!(declared, DataType::Any) {
        return declared;
    }
    match value {
        StaticValue::Float(_) => DataType::Float,
        StaticValue::Int(_) => DataType::Int,
        StaticValue::Bool(_) => DataType::Bool,
        StaticValue::String(_) => DataType::String,
        StaticValue::Null
        | StaticValue::FsPath(_)
        | StaticValue::FsPaths(_)
        | StaticValue::Enum(_) => DataType::Any,
    }
}
