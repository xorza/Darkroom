#![allow(dead_code)]

pub mod common;
pub mod context;
pub mod data;
pub mod elements;
pub mod event;
pub mod execution_graph;
pub mod function;
pub mod graph;
pub mod macros;
pub mod worker;

pub mod prelude {
    pub use crate::context::ContextType;
    pub use crate::data::{DataType, DynamicValue, StaticValue, TypeId};
    pub use crate::execution_graph::{
        ExecutionGraph, ExecutionInput, ExecutionNode, ExecutionOutput, ExecutionStats, InputState,
    };
    pub use crate::function::{
        Func, FuncBehavior, FuncId, FuncLambda, FuncLib, InvokeCache, InvokeError, InvokeInput,
        InvokeResult, TestFuncHooks, test_func_lib,
    };
    pub use crate::graph::{
        Binding, Event, Graph, Input, Node, NodeBehavior, NodeId, PortAddress, test_graph,
    };
}
