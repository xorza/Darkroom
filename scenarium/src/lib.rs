#![forbid(unsafe_code)]
// Lints of the workspace set that stay `allow` there until lumos is swept (plan 2.2);
// this crate is clean for them, so they warn here.
#![warn(
    unused_macro_rules,
    clippy::allow_attributes_without_reason,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::ignore_without_reason,
    clippy::items_after_statements,
    clippy::large_stack_arrays,
    clippy::large_types_passed_by_value,
    clippy::let_underscore_must_use,
    clippy::map_err_ignore,
    clippy::match_same_arms,
    clippy::missing_fields_in_debug,
    clippy::needless_pass_by_value,
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::should_panic_without_expect,
    clippy::struct_field_names,
    clippy::trivially_copy_pass_by_ref,
    clippy::unused_result_ok,
    clippy::unused_self
)]

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

pub use common::CancelToken;
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
pub use execution::seeds::RunSeeds;
pub use graph::Binding;
pub use graph::BindingEntry;
pub use graph::Graph;
pub use graph::NodeRef;
pub use graph::Subscription;
pub use graph::detached::DetachedNode;
pub use graph::error::GraphValidationError;
pub use graph::func::error::{FuncValidationError, InvokeError, InvokeResult};
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
