#![allow(dead_code)]

pub mod common;
pub mod context;
pub mod data;
pub mod elements;
pub mod execution_graph;
pub mod function;
pub mod graph;
pub mod lambda;
pub mod macros;
pub mod worker;

pub mod prelude {
    pub use crate::context::ContextType;
    pub use crate::data::{DataType, DynamicValue, StaticValue, TypeId};
    pub use crate::execution_graph::{
        ExecutionGraph, ExecutionInput, ExecutionNode, ExecutionOutput, ExecutionStats, InputState,
    };
    pub use crate::function::{
        Func, FuncBehavior, FuncId, FuncLib, InvokeCache, TestFuncHooks, test_func_lib,
    };
    pub use crate::graph::{
        Binding, Event, Graph, Input, Node, NodeBehavior, NodeId, PortAddress, test_graph,
    };
    pub use crate::lambda::{FuncLambda, InvokeError, InvokeInput, InvokeResult};
}
