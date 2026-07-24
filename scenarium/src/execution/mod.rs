//! Node-graph execution as an explicit three-phase pipeline:
//!
//! 1. **compile** — the [`Compiler`](compile::Compiler) flattens the authoring
//!    `Graph` into an immutable [`ExecutionProgram`](program::ExecutionProgram).
//!    Runs on the *host's* thread (compile errors are synchronous); the resulting
//!    [`CompiledGraph`](compile::CompiledGraph) is installed into the engine
//!    via [`engine::ExecutionEngine::install`], which cannot fail.
//! 2. **plan** — the [`Planner`](plan::Planner) turns the program into an
//!    [`ExecutionPlan`](plan::ExecutionPlan) (the schedule). Purely structural —
//!    reachability + topological order + missing-input verdicts, no cache/digest state.
//! 3. **execute** — [`RunResourceStamps`](resource::RunResourceStamps) prepares external
//!    identities on the blocking pool; the [`Resolver`](resolve::Resolver) stamps content
//!    digests, then derives cache-aware liveness, exact output demand, and reader counts in
//!    one consumer-first sweep. The [`Executor`](executor::Executor) walks the surviving
//!    schedule producer-first.

pub(crate) mod cache;
pub(crate) mod codec;
pub(crate) mod compile;
pub(crate) mod digest;
pub(crate) mod disk_store;
pub(crate) mod engine;
pub(crate) mod error;
pub(crate) mod event;
pub(crate) mod executor;
mod flatten;
pub(crate) mod identity;
pub(crate) mod outcome;
pub(crate) mod plan;
pub(crate) mod program;
pub(crate) mod report;
pub(crate) mod resolve;
pub(crate) mod resource;
pub(crate) mod seeds;
pub(crate) mod validate;
