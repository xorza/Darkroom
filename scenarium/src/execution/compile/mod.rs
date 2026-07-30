//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: validate the authoring [`Graph`] against the [`Library`], walk it
//! into a flat program — [`flatten`](crate::execution::flatten), which owns
//! both the walk and the [`FlatGraph`] it produces — and link that into a
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
use crate::execution::flatten::Flattener;
use crate::execution::flatten::flat::FlatGraph;
use crate::graph::Graph;
use crate::library::Library;

/// The compile entry point, owning every buffer a compile would otherwise
/// allocate: the `Flattener`'s traversal scratch, the `FlatGraph` the two stages
/// meet over, and the `Linker`'s scratch.
///
/// Hosts keep one per compile site (e.g. darkroom's `Engine`), so an editor that
/// recompiles per edit pays for the artifact and nothing else. The produced
/// [`CompiledGraph`] is always fresh and can be shared with the worker in an
/// [`Arc`](std::sync::Arc) — it is the one thing here that cannot be reused,
/// since the engine, its runtime cache, and the GUI all hold handles to the
/// previous one while the next compile runs.
#[derive(Debug, Default)]
pub struct Compiler {
    flattener: Flattener,
    /// The stage boundary: flatten fills it, link empties it. Private, which is
    /// what replaces the by-value proof that a link consumes exactly one
    /// flatten's output.
    flat: FlatGraph,
    linker: Linker,
}

impl Compiler {
    /// Compile `graph` against `library`: validate, flatten into a func-only
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
        // input, and a passing check lets the flatten pass resolve every
        // reference infallibly.
        if let Err(e) = graph.validate_with(library) {
            tracing::error!(error = %e, "compile rejected: invalid graph");
            return Err(CompileError {
                message: e.to_string(),
            });
        }

        // Walk straight into execution nodes — no intermediate `Graph`.
        self.flattener.flatten(graph, library, &mut self.flat);
        let mut compiled = CompiledGraph::default();
        self.linker.link(&self.flat, library, &mut compiled);

        validate::validate_debug(&compiled, library);
        Ok(compiled)
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use std::sync::Arc;

    use crate::execution::compile::link::Linker;
    use crate::execution::compiled::CompiledGraph;
    use crate::execution::flatten::flat::internals::FlatGraphBuilder;
    use crate::execution::identity::ExecutionNodeId;
    use crate::graph::identity::NodeId;
    use crate::library::Library;

    /// A [`CompiledGraph`] of bare nodes, for a host test that only has to
    /// resolve authored ids against a program. It goes through the real link —
    /// the pipeline's own entry point, so the fixture cannot drift from it.
    #[derive(Debug, Default)]
    pub struct CompiledGraphBuilder {
        flat: FlatGraphBuilder,
    }

    impl CompiledGraphBuilder {
        pub fn new() -> Self {
            Self::default()
        }

        /// Add the execution node an authored node became. One node per
        /// authored id now that nothing dissolves, so the two are the same
        /// identity in different types.
        pub fn insert_node(&mut self, node_id: NodeId) {
            self.flat.insert_node(ExecutionNodeId::from_node(node_id));
        }

        pub fn build(self) -> Arc<CompiledGraph> {
            let mut compiled = CompiledGraph::default();
            // These nodes declare no ports, so linking never reaches for a
            // declaration and an empty library answers for all of them.
            Linker::default().link(&self.flat.build(), &Library::default(), &mut compiled);
            Arc::new(compiled)
        }
    }
}

#[cfg(test)]
mod tests;
