#![allow(dead_code)]

#[cfg(test)]
mod tests;

#[cfg(feature = "opencl")]
pub mod ocl_context;

pub mod common;
pub mod runtime;
pub mod graph;
pub mod functions;
pub mod invoke;
pub mod lua_invoker;
pub mod data_type;
pub mod c_api;
