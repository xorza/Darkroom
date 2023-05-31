#![allow(dead_code)]

mod runtime_tests;
mod graph_tests;
mod lua_invoker_tests;

#[cfg(feature = "opencl")]
mod ocl_tests;
#[cfg(feature = "opencl")]
pub mod ocl_context;

pub mod common;
pub mod runtime;
pub mod graph;
pub mod function_graph;
pub mod invoke;
pub mod lua_invoker;
pub mod data_type;
