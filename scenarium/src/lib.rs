#![forbid(unsafe_code)]

mod builtin;
mod containers;
mod data;
mod decl;
mod exec;
mod graph;
mod identity;
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
pub use data::data_type::type_ref::TypeRef;
pub use data::data_type::{DataType, FileMode};
pub use data::dynamic_value::{CustomValue, DynamicValue};
pub use data::path_form::PathForm;
pub use data::ram_usage::RamUsage;
pub use decl::cache_mode::{CacheMode, Keep};
pub use decl::func::error::{FuncValidationError, InvokeError, InvokeResult};
pub use decl::func::event_lambda::{EventFires, EventLambda};
pub use decl::func::func_builder::FuncBuilder;
pub use decl::func::func_lambda::{
    AsyncLambda, AsyncLambdaFn, FuncLambda, Invocation, OutputDemand,
};
pub use decl::func::{Func, FuncBehavior};
pub use decl::port_map::{NodeRemap, PortMap, PortMapError};
pub use decl::ports::error::{BindingRejection, PortList, PortsError};
pub use decl::ports::{EventDecl, InputDecl, Numbered, OutputDecl, Ports};
pub use decl::saved_decl::{SavedDecl, SavedEvent, SavedInput, SavedPort, ShownPort};
pub use decl::value_domain::ValueDomain;
pub use decl::value_domain::float_span::FloatSpan;
pub use decl::value_domain::int_span::IntSpan;
pub use decl::value_domain::span_error::SpanError;
pub use decl::value_domain::value_variant::ValueVariant;
pub use exec::cache::disk_store::DiskStore;
pub use exec::cache::disk_store::error::{
    CodecFormatError, FrameError, PruneError, RemovalError, StoreError,
};
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
pub use graph::compound::{Compound, CompoundDecl, CompoundRef};
pub use graph::detached::{DetachedGhost, DetachedNode, DetachedNodeParts};
pub use graph::error::{
    BindRejection, DetachReason, DetachedNodeError, GhostSubscriptionRejection,
    GraphSetValidationError, GraphValidationError, PlacementRejection, SubscriptionRejection,
    Unresolved,
};
pub use graph::ghost_node::GhostNode;
pub use graph::ghost_wiring::{
    GhostBinding, GhostSource, GhostSubscription, GhostWiring, InputState, SubscriptionState,
};
pub use graph::graph_set::GraphSet;
pub use graph::graph_set::digest::CompoundDigest;
pub use graph::graph_set::program_shape::ProgramShape;
pub use graph::graph_set::unknown_type::{InterfacePort, UnknownType};
pub use graph::interface::EventPorts;
pub use graph::interface::Interface;
pub use graph::interface_change::{ChangeSide, InterfaceChange, RefusedFate, WiringChange};
pub use graph::manifest::Manifest;
pub use graph::node::special::{SPECIAL_NODES, SpecialNode};
pub use graph::node::{Boundary, Node};
pub use graph::node_decl::NodeDecl;
pub use graph::node_decl::NodeTriggers;
pub use graph::node_ref::NodeRef;
pub use graph::reconciled::{GraphSetReconciled, Repair};
pub use graph::scope::Scope;
pub use graph::subscription::Subscription;
pub use graph::type_names::TypeNames;
pub use graph::wiring::Wiring;
pub use identity::instance_path::{InstancePath, InstancePathError};
pub use identity::program_id::{ProgramId, ProgramIdError};
pub use identity::{
    CompoundId, EventPort, FuncId, GraphId, InputPort, NodeAddr, NodeId, OutputPort, PortId,
    TriggerPort,
};
pub use library::Library;
pub use library::func_redirect::{FuncRedirect, MAX_REDIRECTS};
pub use library::library_builder::LibraryBuilder;
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
