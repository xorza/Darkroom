//! Node-graph execution as an explicit three-phase pipeline:
//!
//! 1. **compile** — the [`Compiler`](compile::Compiler) runs the authoring `Graph`
//!    through [`flatten`], then links the result into an immutable
//!    [`CompiledGraph`](compiled::CompiledGraph). Runs on the *host's* thread
//!    (compile errors are synchronous); the artifact is installed into the
//!    engine via [`engine::ExecutionEngine::install`], which cannot fail.
//! 2. **plan** — the [`Planner`](schedule::planner::Planner) turns the program into a
//!    [`RunSchedule`](schedule::RunSchedule). Purely structural —
//!    reachability + topological order + missing-input verdicts, no cache/digest state.
//! 3. **execute** — the [`RuntimeCache`](cache::runtime::RuntimeCache) prepares filesystem
//!    identities on the blocking pool; [`resolve`](schedule::Scheduled::resolve) stamps
//!    content digests, then derives cache-aware liveness, exact output demand, and reader
//!    counts in one consumer-first sweep, refining that same schedule. The
//!    [`Executor`](executor::Executor) walks the surviving schedule producer-first.

pub(crate) mod cache;
pub(crate) mod compile;
pub(crate) mod compiled;
pub(crate) mod engine;
pub(crate) mod error;
pub(crate) mod executor;
pub(crate) mod flatten;
pub(crate) mod identity;
pub(crate) mod report;
pub(crate) mod schedule;
pub(crate) mod seeds;
