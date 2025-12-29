#![allow(dead_code)]
#![allow(unused_imports)]

pub mod common;
pub mod compute;
pub mod data;
pub mod elements;
pub mod event;
pub mod execution_graph;
pub mod function;
pub mod graph;
pub mod worker;

pub mod prelude {
    pub use crate::data::{DataType, DynamicValue, StaticValue, TypeId};
    pub use crate::execution_graph::{
        ExecutionGraph, ExecutionGraphError, ExecutionInput, ExecutionNode, ExecutionOutput,
        InputState, PortAddress,
    };
    pub use crate::function::{
        Func, FuncBehavior, FuncId, FuncLambda, InvokeArgs, InvokeCache, InvokeError, InvokeResult,
    };
    pub use crate::graph::{
        Binding, Event, Graph, Input, Node, NodeBehavior, NodeId, OutputBinding,
    };
}
