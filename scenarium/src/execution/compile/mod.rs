//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: validate the authoring [`Graph`] against the [`Library`], dissolve
//! its composites ([`flatten`]), and link the result into a self-contained
//! [`CompiledGraph`] the worker installs as-is. Compile
//! errors surface synchronously at the call site — a graph that doesn't
//! compile is never sent, so the worker's install is infallible and a running
//! event loop is never disturbed by a bad edit.

pub(crate) mod error;
mod flat;
mod flatten;
mod link;
mod validate;

use crate::execution::compile::error::CompileError;

use crate::execution::compile::flatten::Flattener;
use crate::execution::compiled::CompiledGraph;
use crate::graph::Graph;
use crate::library::Library;

/// The compile entry point, owning reusable `Flattener` traversal scratch.
/// Hosts keep one per compile site (e.g. darkroom's `Engine`); the produced
/// [`CompiledGraph`] is always fresh and can be shared with the worker in an
/// [`Arc`](std::sync::Arc).
#[derive(Debug, Default)]
pub struct Compiler {
    flattener: Flattener,
}

impl Compiler {
    /// Compile `graph` against `library`: validate, flatten composites into a
    /// flat func-only program, and resolve the output-type pool. Pure CPU on
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

        // Flatten graphs straight into execution nodes — no intermediate
        // `Graph`. Everything downstream is boundary-agnostic (func nodes only),
        // and carries what it needs out of the library, so linking takes the
        // flat graph alone.
        let compiled = link::link(self.flattener.flatten(graph, library));
        validate::validate_debug(&compiled, library);
        Ok(compiled)
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use std::sync::Arc;

    use crate::execution::compile::flat::internals::FlatGraphBuilder;
    use crate::execution::compiled::CompiledGraph;
    use crate::execution::identity::ExecutionNodeId;
    use crate::graph::identity::NodeId;

    /// A [`CompiledGraph`] carrying attribution and nothing else, for a host
    /// test that only has to project execution ids onto authored ones. It goes
    /// through the real link, over a flat graph of bare nodes — the pipeline's
    /// own entry point, so the fixture cannot drift from it.
    #[derive(Debug, Default)]
    pub struct CompiledGraphBuilder {
        flat: FlatGraphBuilder,
    }

    impl CompiledGraphBuilder {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn insert_leaf(
            &mut self,
            e_node_id: ExecutionNodeId,
            instances: impl IntoIterator<Item = NodeId>,
            node_id: NodeId,
        ) {
            self.flat.insert_leaf(e_node_id, instances, node_id);
        }

        pub fn build(self) -> Arc<CompiledGraph> {
            Arc::new(super::link::link(self.flat.build()))
        }
    }
}

#[cfg(test)]
mod tests;
