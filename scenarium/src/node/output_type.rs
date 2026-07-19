use std::hash::Hash;

use hashbrown::HashMap;

use crate::{DataType, StaticValue};

#[derive(Debug)]
pub(crate) enum OutputTypeSource<K> {
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
pub(crate) struct OutputTypeResolver<K> {
    states: HashMap<K, ResolutionState>,
    path: Vec<K>,
}

impl<K> OutputTypeResolver<K> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            states: HashMap::with_capacity(capacity),
            path: Vec::new(),
        }
    }
}

impl<K> OutputTypeResolver<K>
where
    K: Copy + Eq + Hash,
{
    pub(crate) fn resolve(
        &mut self,
        output: K,
        source: &impl Fn(K) -> OutputTypeSource<K>,
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
                OutputTypeSource::Fixed(data_type) => break data_type,
                OutputTypeSource::Bind(bound) => current = bound,
                OutputTypeSource::Const { declared, value } => {
                    break constant_output_type(declared, value);
                }
                OutputTypeSource::Unresolved => break DataType::Any,
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
        StaticValue::Null | StaticValue::FsPath(_) | StaticValue::Enum(_) => DataType::Any,
    }
}
