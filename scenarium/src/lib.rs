#![allow(dead_code)]

pub mod common;
pub mod context;
pub mod data;
pub mod elements;
pub mod event_lambda;
pub mod execution_graph;
pub mod execution_stats;
pub mod func_lambda;
pub mod function;
pub mod graph;
pub mod macros;
pub mod worker;

pub mod prelude {
    pub use crate::common::any_state::AnyState;
    pub use crate::common::shared_any_state::SharedAnyState;
    pub use crate::context::ContextType;
    pub use crate::data::{CustomValue, DataType, DynamicValue, StaticValue, TypeId};
    pub use crate::execution_graph::{
        ExecutionGraph, ExecutionInput, ExecutionNode, ExecutionOutput, InputState,
    };
    pub use crate::execution_stats::{ExecutedNodeStats, ExecutionStats, NodeError};
    pub use crate::func_lambda::{FuncLambda, InvokeError, InvokeInput, InvokeResult};
    pub use crate::function::{Func, FuncBehavior, FuncId, FuncLib, TestFuncHooks, test_func_lib};
    pub use crate::graph::{
        Binding, Event, Graph, Input, Node, NodeBehavior, NodeId, PortAddress, test_graph,
    };
}
