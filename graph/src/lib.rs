#![allow(dead_code)]
#![allow(unused_imports)]

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
        test_func_lib, Func, FuncBehavior, FuncId, FuncLambda, FuncLib, InvokeCache, InvokeError,
        InvokeInput, InvokeResult, TestFuncHooks,
    };
    pub use crate::graph::{
        test_graph, Binding, Event, Graph, Input, Node, NodeBehavior, NodeId, PortAddress,
    };
}
