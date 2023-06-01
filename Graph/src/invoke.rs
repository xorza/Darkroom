use crate::data_type::DataType;
use std::collections::HashMap;
use std::ops::{Index, IndexMut};
use uuid::Uuid;

#[derive(Clone, PartialEq)]
pub enum Value {
    Null,
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
}

#[derive(Clone, Default)]
pub struct Args {
    values: Vec<Value>,
}

pub trait Invoker {
    fn start(&self) {}
    fn call(&self, function_name: &str, context_id: Uuid, inputs: &Args, outputs: &mut Args);
    fn finish(&self) {}
}

pub trait Invokable {
    fn call(&self, context_id: Uuid, inputs: &Args, outputs: &mut Args);
}

pub struct LambdaInvokable {
    lambda: Box<dyn Fn(Uuid, &Args, &mut Args)>,
}

pub struct LambdaInvoker {
    lambdas: HashMap<String, LambdaInvokable>,
}

impl LambdaInvoker {
    pub fn new() -> LambdaInvoker {
        LambdaInvoker {
            lambdas: HashMap::new(),
        }
    }

    pub fn add_lambda<F: Fn(Uuid, &Args, &mut Args) + 'static>(
        &mut self,
        function_name: &str,
        lambda: F,
    ) {
        let invokable = LambdaInvokable {
            lambda: Box::new(lambda),
        };
        self.lambdas.insert(function_name.to_string(), invokable);
    }
}

impl Invoker for LambdaInvoker {
    fn start(&self) {}
    fn call(&self, function_name: &str, context_id: Uuid, inputs: &Args, outputs: &mut Args) {
        if let Some(func) = self.lambdas.get(function_name) {
            (func.lambda)(context_id, inputs, outputs);
        } else {
            panic!("Function not found: {}", function_name);
        }
    }
    fn finish(&self) {}
}

impl Value {
    pub fn data_type(&self) -> DataType {
        match self {
            Value::Null => DataType::None,
            Value::Float(_) => DataType::Float,
            Value::Int(_) => DataType::Int,
            Value::Bool(_) => DataType::Bool,
            Value::String(_) => DataType::String,
        }
    }

    pub fn as_float(&self) -> f64 {
        match self {
            Value::Float(value) => *value,
            _ => {
                panic!("Value is not a float")
            }
        }
    }
    pub fn as_int(&self) -> i64 {
        match self {
            Value::Int(value) => *value,
            _ => {
                panic!("Value is not an int")
            }
        }
    }
    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(value) => *value,
            _ => {
                panic!("Value is not a bool")
            }
        }
    }
    pub fn as_string(&self) -> &str {
        match self {
            Value::String(value) => value,
            _ => {
                panic!("Value is not a string")
            }
        }
    }
}

impl Args {
    pub fn new() -> Args {
        Args { values: Vec::new() }
    }
    pub fn with_size(size: usize) -> Args {
        let mut result = Args {
            values: Vec::with_capacity(size),
        };
        result.values.resize(size, Value::Null);
        return result;
    }

    fn resize(&mut self, size: usize) {
        self.values.resize(size, Value::Null);
    }

    pub fn iter(&self) -> std::slice::Iter<Value> {
        self.values.iter()
    }

    pub fn from_vec<T: Into<Value>>(values: Vec<T>) -> Args {
        let mut result = Args::new();
        for value in values {
            result.values.push(value.into());
        }

        result
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }
}

impl Index<usize> for Args {
    type Output = Value;

    fn index(&self, idx: usize) -> &Value {
        &self.values[idx]
    }
}

impl IndexMut<usize> for Args {
    fn index_mut(&mut self, index: usize) -> &mut Value {
        &mut self.values[index]
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Value::Int(value as i64)
    }
}

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Value::Float(value as f64)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value as f64)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::String(value.to_string())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value as bool)
    }
}

impl From<DataType> for Value {
    fn from(data_type: DataType) -> Self {
        match data_type {
            DataType::None => Value::Null,
            DataType::Float => Value::Float(0.0),
            DataType::Int => Value::Int(0),
            DataType::Bool => Value::Bool(false),
            DataType::String => Value::Null,
        }
    }
}
