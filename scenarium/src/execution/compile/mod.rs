//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: validate the authoring [`Graph`] against the [`Library`], walk it
//! into a flat program — [`lower`](crate::execution::lower), which owns
//! both the walk and the [`LoweredGraph`] it produces — and link that into a
//! self-contained [`CompiledGraph`] the worker installs as-is. Compile
//! errors surface synchronously at the call site — a graph that doesn't
//! compile is never sent, so the worker's install is infallible and a running
//! event loop is never disturbed by a bad edit.

pub(crate) mod error;
mod link;
mod validate;

use crate::execution::compile::error::CompileError;

use crate::execution::compile::link::Linker;
use crate::execution::compiled::CompiledGraph;
use crate::execution::lower::Lowerer;
use crate::execution::lower::lowered_graph::LoweredGraph;
use crate::graph::Graph;
use crate::library::Library;

/// The compile entry point, owning every buffer a compile would otherwise
/// allocate: the `Lowerer`'s traversal scratch, the `LoweredGraph` the two
/// stages meet over, and the `Linker`'s scratch.
///
/// Hosts keep one per compile site (e.g. darkroom's `Engine`), so an editor that
/// recompiles per edit pays for the artifact and nothing else. The produced
/// [`CompiledGraph`] is always fresh and can be shared with the worker in an
/// [`Arc`](std::sync::Arc) — it is the one thing here that cannot be reused,
/// since the engine, its runtime cache, and the GUI all hold handles to the
/// previous one while the next compile runs.
#[derive(Debug, Default)]
pub struct Compiler {
    lowerer: Lowerer,
    /// The stage boundary: lowering fills it, link empties it. Private, which is
    /// what replaces the by-value proof that a link consumes exactly one
    /// lowering's output.
    lowered: LoweredGraph,
    linker: Linker,
}

impl Compiler {
    /// Compile `graph` against `library`: validate, lower into a func-only
    /// program, and resolve the output-type pool. Pure CPU on
    /// the caller's thread; the result is
    /// installed into an engine (`ExecutionEngine::install`)
    /// (typically across the worker channel).
    pub fn compile(
        &mut self,
        graph: &Graph,
        library: &Library,
    ) -> Result<CompiledGraph, CompileError> {
        // Validate before building anything: the graph+library pair is untrusted
        // input, and a passing check lets the lowering pass resolve every
        // reference infallibly.
        if let Err(e) = graph.validate_with(library) {
            tracing::error!(error = %e, "compile rejected: invalid graph");
            return Err(CompileError {
                message: e.to_string(),
            });
        }

        // Walk straight into execution nodes — no intermediate `Graph`.
        self.lowerer.lower(graph, library, &mut self.lowered);
        let mut compiled = CompiledGraph::default();
        self.linker.link(&self.lowered, library, &mut compiled);

        validate::validate_debug(&compiled, library);
        Ok(compiled)
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use std::sync::Arc;

    use crate::execution::compile::link::Linker;
    use crate::execution::compiled::CompiledGraph;
    use crate::execution::lower::lowered_graph::internals::LoweredGraphBuilder;
    use crate::graph::identity::NodeId;
    use crate::library::Library;

    /// A [`CompiledGraph`] of bare nodes, for a host test that only has to
    /// resolve authored ids against a program. It goes through the real link —
    /// the pipeline's own entry point, so the fixture cannot drift from it.
    #[derive(Debug, Default)]
    pub struct CompiledGraphBuilder {
        lowered: LoweredGraphBuilder,
    }

    impl CompiledGraphBuilder {
        pub fn new() -> Self {
            Self::default()
        }

        /// Add the execution node an authored node became. One node per
        /// authored id now that nothing dissolves, so the two are the same
        /// identity in different types.
        pub fn insert_node(&mut self, node_id: NodeId) {
            self.lowered.insert_node(node_id);
        }

        pub fn build(self) -> Arc<CompiledGraph> {
            let mut compiled = CompiledGraph::default();
            // These nodes declare no ports, so linking never reaches for a
            // declaration and an empty library answers for all of them.
            Linker::default().link(&self.lowered.build(), &Library::default(), &mut compiled);
            Arc::new(compiled)
        }
    }
}

#[cfg(test)]
mod tests;
