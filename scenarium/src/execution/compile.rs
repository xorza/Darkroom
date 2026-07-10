//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: validate + flatten the authoring [`Graph`] against the [`Library`]
//! into a self-contained [`CompiledGraph`] the worker installs as-is. Compile
//! errors surface synchronously at the call site — a graph that doesn't
//! compile is never sent, so the worker's install is infallible and a running
//! event loop is never disturbed by a bad edit.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::execution::flatten::{Flattener, Pools};
use crate::execution::program::ExecutionProgram;
use crate::execution::stats::FlattenMap;
use crate::graph::Graph;
use crate::library::Library;

/// The graph won't compile against the library: a document can be stale
/// against an evolved library (a dropped func, a shrunk port list, a
/// type-mismatched binding), so this is a recoverable error the caller
/// surfaces, not a logic bug. The compile-phase counterpart of the run-phase
/// [`Error`](crate::execution::Error) — the two can't be confused at the type
/// level, and only `compile` produces it.
#[derive(Debug, Error, Clone, Serialize, Deserialize)]
#[error("invalid graph: {message}")]
pub struct CompileError {
    pub message: String,
}

/// The compile artifact: the flattened, immutable program (lambdas, resolved
/// output types, and input stampers attached) plus the [`FlattenMap`] that
/// projects flat ids back onto authoring nodes (node-seed resolution, stats
/// attribution). Self-contained — executing it needs neither the authoring
/// graph nor the library. `Default` is the empty program (the engine's
/// pre-install / cleared state).
#[derive(Debug, Default)]
pub struct CompiledGraph {
    pub(crate) program: ExecutionProgram,
    pub(crate) flatten_map: Arc<FlattenMap>,
}

/// The compile entry point, owning the reusable [`Flattener`] scratch so the
/// flatten buffers aren't reallocated on every compile. Hosts keep one per
/// compile site (e.g. darkroom's `Engine`); the produced [`CompiledGraph`] is
/// always fresh — it's moved to the worker.
#[derive(Debug, Default)]
pub struct Compiler {
    flattener: Flattener,
}

impl Compiler {
    /// Compile `graph` against `library`: validate, flatten composites into a
    /// flat func-only program, and resolve the output-type pool. Pure CPU on
    /// the caller's thread; the result is
    /// [installed](crate::execution::ExecutionEngine::install) into an engine
    /// (typically across the worker channel).
    pub fn compile(
        &mut self,
        graph: &Graph,
        library: &Library,
    ) -> Result<CompiledGraph, CompileError> {
        // Validate before building anything: the graph+library pair is untrusted
        // input, and a passing check lets the flatten pass resolve every
        // reference infallibly.
        if let Err(e) = graph.check_with(library) {
            tracing::error!(error = %e, "compile rejected: invalid graph");
            return Err(CompileError {
                message: e.to_string(),
            });
        }

        // Flatten subgraphs straight into execution nodes — no intermediate
        // `Graph`. Everything downstream is boundary-agnostic (func nodes only).
        let mut program = ExecutionProgram::default();
        let mut flatten_map = FlattenMap::default();
        self.flattener.build(
            &mut program.e_nodes,
            Pools {
                inputs: &mut program.inputs,
                events: &mut program.events,
            },
            graph,
            library,
            &mut flatten_map,
        );

        // Resolve the program's output-type pool from the full library (every func
        // is present — `check_with` validated them), making the compiled program
        // self-describing: the digest and the disk cache's codec check read it
        // with no library at run time.
        program.resolve_output_types(library);

        let compiled = CompiledGraph {
            program,
            flatten_map: Arc::new(flatten_map),
        };
        compiled.validate(library);
        Ok(compiled)
    }
}
