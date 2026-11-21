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
pub use graph::compound::Compound;
pub use graph::detached::{DetachedGhost, DetachedNode, DetachedNodeParts};
pub use graph::error::{
    BindingRejection, DetachReason, DetachedNodeError, GhostRejection, GhostSubscriptionRejection,
    GraphSetValidationError, GraphValidationError, PlacementRejection, SubscriptionRejection,
    Unresolved,
};
pub use graph::func::error::{FuncValidationError, InvokeError, InvokeResult};
pub use graph::func::event::{EventFires, EventLambda};
pub use graph::func::func_builder::FuncBuilder;
pub use graph::func::lambda::{AsyncLambda, AsyncLambdaFn, FuncLambda, Invocation, OutputDemand};
pub use graph::func::value_variant::ValueVariant;
pub use graph::func::{Func, FuncBehavior, FuncEvent};
pub use graph::ghost_node::GhostNode;
pub use graph::ghost_wiring::{
    GhostBinding, GhostSource, GhostSubscription, GhostWiring, InputState, SubscriptionState,
};
pub use graph::graph_set::digest::CompoundDigest;
pub use graph::graph_set::{GraphSet, InterfacePort, ProgramShape, UnknownType};
pub use graph::identity::{
    CompoundId, EventPort, FuncId, GraphId, InputPort, NodeAddr, NodeId, OutputPort, PortId,
    TriggerPort,
};
pub use graph::instance_path::{InstancePath, InstancePathError};
pub use graph::interface::EventPorts;
pub use graph::interface::Interface;
pub use graph::interface_change::{
    ChangeSide, InterfaceChange, NamedGhost, RefusedBinding, RefusedFate, RefusedSubscription,
};
pub use graph::manifest::Manifest;
pub use graph::node::special::{SPECIAL_NODES, SpecialNode};
pub use graph::node::{Boundary, CacheMode, Keep, Node};
pub use graph::node_decl::NodeDecl;
pub use graph::node_decl::{EventRef, NodeEvents, NodeTriggers};
pub use graph::node_ref::NodeRef;
pub use graph::port_map::{NodeRemap, PortMap, PortMapError};
pub use graph::ports::{EventDecl, InputDecl, Numbered, OutputDecl, PortList, Ports, PortsError};
pub use graph::program_id::{ProgramId, ProgramIdError};
pub use graph::reconciled::{GraphSetReconciled, Reconciled, Repair};
pub use graph::saved_decl::{SavedDecl, SavedEvent, SavedInput, SavedPort, ShownPort};
pub use graph::scope::Scope;
pub use graph::subscription::Subscription;
pub use graph::type_names::{TypeNames, TypeRef};
pub use graph::value_domain::ValueDomain;
pub use graph::value_domain::float_span::FloatSpan;
pub use graph::value_domain::int_span::IntSpan;
pub use graph::value_domain::span_error::SpanError;
pub use library::Library;
pub use library::func_redirect::{FuncRedirect, MAX_REDIRECTS};
pub use runtime::any_state::AnyState;
pub use runtime::ctx_manager::CtxManager;
pub use runtime::log_entry::{LogEntry, LogLevel};
pub use runtime::shared_any_state::{EventStateGuard, SharedAnyState};
pub use worker::Worker;
pub use worker::activity::WorkerActivity;
pub use worker::error::{ExitError, WorkerError, WorkerExited};
pub use worker::protocol::{WorkerMessage, WorkerReport};
pub use worker::run_summary::RunSummary;

/// The allocator the allocation tests count through.
#[cfg(test)]
mod counting_allocator {
    use common::internals::alloc_audit::CountingAllocator;

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;
}
