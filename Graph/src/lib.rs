#![allow(dead_code)]

#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
mod graph_tests;
#[cfg(test)]
mod lua_invoker_tests;
#[cfg(all(feature = "opencl", test))]
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
