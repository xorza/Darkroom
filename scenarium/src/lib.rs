#![forbid(unsafe_code)]

mod builtin;
mod containers;
mod data;
mod exec;
mod graph;
#[cfg(feature = "internals")]
pub mod internals;
mod library;
mod runtime;
mod worker;

pub use builtin::math_library::math_library;
pub use builtin::system_library::system_library;
pub use builtin::worker_events_library::{FRAME_EVENT_FUNC_ID, worker_events_library};
pub use data::codec::CustomValueCodec;
pub use data::codec::error::CodecError;
pub use data::const_value::{ConstValue, ValueText};
pub use data::data_type::custom_type::{CustomType, CustomTypeId};
pub use data::data_type::enum_type::{EnumId, EnumType};
pub use data::data_type::file_kind::{FileKind, FileKindId};
pub use data::data_type::{DataType, EnumVariants, FileMode};
pub use data::dynamic_value::{CustomValue, DynamicValue};
pub use data::path_form::PathForm;
pub use data::ram_usage::RamUsage;
pub use exec::cache::disk_store::DiskStore;
pub use exec::cache::disk_store::error::{CodecFormatError, FrameError, RemovalError, StoreError};
pub use exec::cache::runtime_cache::error::{
    CacheFlushUnsupported, CacheNodeError, CacheNodeFailure,
};
pub use exec::compile::Compiler;
pub use exec::compile::compiled_graph::CompiledGraph;
pub use exec::error::{ExecError, ExecResult, RunError};
pub use exec::report::node_status::NodeExecStatus;
pub use exec::report::node_status::NodeStatus;
pub use exec::report::run_reporter::RunPhase;
pub use graph::Graph;
pub use graph::binding::Binding;
pub use graph::binding_entry::BindingEntry;
pub use graph::detached::{DetachedNode, DetachedNodeParts};
pub use graph::error::{
    BindingRejection, DetachReason, DetachedNodeError, GraphValidationError, SubscriptionRejection,
    Unresolved,
};
pub use graph::func::error::{FuncValidationError, InvokeError, InvokeResult};
pub use graph::func::event::{EventFires, EventLambda};
pub use graph::func::func_builder::FuncBuilder;
pub use graph::func::lambda::{AsyncLambda, AsyncLambdaFn, FuncLambda, Invocation, OutputDemand};
pub use graph::func::value_variant::ValueVariant;
pub use graph::func::{Func, FuncBehavior, FuncEvent};
pub use graph::identity::{EventPort, FuncId, InputPort, NodeId, OutputPort};
pub use graph::instance_path::{InstancePath, InstancePathError};
pub use graph::node::special::{SPECIAL_NODES, SpecialNode};
pub use graph::node::{CacheMode, Node};
pub use graph::node_ref::NodeRef;
pub use graph::ports::{InputDecl, OutputDecl, Ports};
pub use graph::program_id::{ProgramId, ProgramIdError};
pub use graph::reconciled::{Reconciled, Repair};
pub use graph::stub::Stub;
pub use graph::subscription::Subscription;
pub use library::Library;
pub use runtime::any_state::AnyState;
pub use runtime::ctx_manager::CtxManager;
pub use runtime::log_entry::{LogEntry, LogLevel};
pub use runtime::shared_any_state::{EventStateGuard, SharedAnyState};
pub use worker::Worker;
pub use worker::activity::WorkerActivity;
pub use worker::error::{WorkerError, WorkerExited};
pub use worker::protocol::{WorkerMessage, WorkerReport};
pub use worker::run_summary::RunSummary;

/// The allocator the allocation tests count through.
#[cfg(test)]
mod counting_allocator {
    use common::internals::alloc_audit::CountingAllocator;

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;
}
