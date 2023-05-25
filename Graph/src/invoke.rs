use std::collections::HashMap;
use std::ops::{Index, IndexMut};
use crate::data_type::DataType;

#[derive(Clone, PartialEq)]
pub enum Value {
    Null,
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),

}

#[derive(Clone)]
pub struct Args {
    values: Vec<Value>,
}

pub trait Invoker {
    fn start(&self) {}
    fn call(&self, function_name: &str, context_id: u32, inputs: &Args, outputs: &mut Args);
    fn finish(&self) {}
}

pub trait Invokable {
    fn call(&self, context_id: u32, inputs: &Args, outputs: &mut Args);
}

pub struct LambdaInvokable {
    lambda: Box<dyn Fn(u32, &Args, &mut Args)>,
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

    pub fn add_lambda<F: Fn(u32, &Args, &mut Args) + 'static>(&mut self, function_name: &str, lambda: F) {
        let invokable = LambdaInvokable {
            lambda: Box::new(lambda),
        };
        self.lambdas.insert(function_name.to_string(), invokable);
    }
}

impl Invoker for LambdaInvoker {
    fn start(&self) {}
    fn call(&self, function_name: &str, context_id: u32, inputs: &Args, outputs: &mut Args) {
        if let Some(func) = self.lambdas.get(function_name) {
            (func.lambda)(context_id, inputs, outputs);
        } else {
            panic!("Function not found: {}", function_name);
        }
    }
    fn finish(&self) {}
}

impl Value {
    pub fn from_int(value: i64) -> Value {
        Value::Int(value)
    }

    pub fn to_int(&self) -> i64 {
        match self {
            Value::Int(value) => *value,
            _ => panic!("Value is not an int"),
        }
    }

    pub fn data_type(&self) -> DataType {
        match self {
            Value::Null => { DataType::None }
            Value::Float(_) => { DataType::Float }
            Value::Int(_) => { DataType::Int }
            Value::Bool(_) => { DataType::Bool }
            Value::String(_) => { DataType::String }
        }
    }
}

impl Args {
    pub fn new() -> Args {
        Args {
            values: Vec::new(),
        }
    }

    pub fn resize(&mut self, size: usize) {
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
impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value as bool)
    }
}
