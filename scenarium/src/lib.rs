#![forbid(unsafe_code)]
// Lints of the workspace set that stay `allow` there until lumos is swept (plan 2.2);
// this crate is clean for them, so they warn here.
#![warn(clippy::cast_possible_wrap, clippy::cast_sign_loss)]

mod containers;
mod data;
mod elements;
mod execution;
mod graph;
mod library;
mod runtime;
#[cfg(any(test, feature = "internals"))]
pub mod testing;
mod worker;

pub use data::codec::CustomValueCodec;
pub use data::codec::error::{CodecError, CodecFormatError};
pub use data::const_value::{ConstValue, ValueText};
pub use data::dynamic_value::{CustomValue, DynamicValue, RamUsage};
pub use data::type_system::{DataType, EnumVariants, FsPathConfig, FsPathMode, TypeId};
pub use elements::math_library::math_library;
pub use elements::system_library::system_library;
pub use elements::worker_events_library::{FRAME_EVENT_FUNC_ID, worker_events_library};
pub use execution::cache::disk_store::DiskStore;
pub use execution::cache::disk_store::error::{RemovalError, StoreError};
pub use execution::cache::runtime::error::{
    CacheFlushUnsupported, CacheNodeError, CacheNodeFailure,
};
pub use execution::compile::Compiler;
pub use execution::compile::compiled_graph::CompiledGraph;
pub use execution::compile::error::CompileError;
pub use execution::error::{Error, Result, RunError};
pub use execution::report::{LogEntry, LogLevel};
pub use execution::report::{NodeExecutionStatus, NodeStatus, RunPhase};
pub use graph::Binding;
pub use graph::BindingEntry;
pub use graph::Graph;
pub use graph::NodeRef;
pub use graph::Subscription;
pub use graph::detached::DetachedNode;
pub use graph::error::{DetachedNodeError, GraphValidationError};
pub use graph::func::error::{FuncValidationError, InvokeError, InvokeResult, OverrideRule};
pub use graph::func::event::{AsyncEvent, AsyncEventFn, EventLambda};
pub use graph::func::lambda::{AsyncLambda, AsyncLambdaFn, FuncLambda, Invocation, OutputDemand};
pub use graph::func::signature::FuncSignature;
pub use graph::func::{
    Func, FuncBehavior, FuncEvent, FuncInput, FuncOutput, OutputType, ValueVariant,
};
pub use graph::identity::{EventPort, FuncId, InputPort, NodeId, OutputPort};
pub use graph::node::special::{SPECIAL_NODES, SpecialNode};
pub use graph::node::{CacheMode, Node, NodeKind};
pub use graph::output_types::OutputTypes;
pub use library::{Library, TypeEntry};
pub use runtime::any_state::AnyState;
pub use runtime::context::ContextManager;
pub use runtime::shared_any_state::{EventStateGuard, SharedAnyState};
pub use worker::Worker;
pub use worker::activity::WorkerActivity;
pub use worker::error::{WorkerError, WorkerExited};
pub use worker::protocol::{WorkerMessage, WorkerReport};
pub use worker::run_summary::RunSummary;
