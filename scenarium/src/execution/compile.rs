//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: validate + flatten the authoring [`Graph`] against the [`Library`]
//! into a self-contained [`CompiledGraph`] the worker installs as-is. Compile
//! errors surface synchronously at the call site — a graph that doesn't
//! compile is never sent, so the worker's install is infallible and a running
//! event loop is never disturbed by a bad edit.

use std::sync::Arc;

use thiserror::Error;

use crate::execution::flatten::{Flattener, Pools};
use crate::execution::identity::{FlattenMap, NodeAddress};
use crate::execution::program::ExecutionProgram;
use crate::graph::{Graph, NodeId};
use crate::library::Library;

/// The graph won't compile against the library: a document can be stale
/// against an evolved library (a dropped func, a shrunk port list, a
/// type-mismatched binding), so this is a recoverable error the caller
/// surfaces, not a logic bug. The compile-phase counterpart of the run-phase
/// [`Error`](crate::execution::Error) — the two can't be confused at the type
/// level, and only `compile` produces it.
#[derive(Debug, Error)]
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

impl CompiledGraph {
    /// Choose an execution address when a host has only a bare authoring node id.
    /// Interior ids use the map's stable representative; an unknown id remains a
    /// root address so execution reports the inconsistent seed.
    pub fn node_address(&self, node_id: NodeId) -> NodeAddress {
        self.flatten_map
            .representative(&node_id)
            .cloned()
            .unwrap_or_else(|| NodeAddress::root(node_id))
    }
}

/// The two ownership handles produced by compilation: an opaque artifact to
/// move to the worker and the same flatten map retained by the host for
/// projecting worker reports back onto authoring nodes.
#[derive(Debug)]
pub struct Compilation {
    pub compiled: CompiledGraph,
    pub flatten_map: Arc<FlattenMap>,
}

/// The compile entry point, owning reusable [`Flattener`] traversal scratch.
/// Hosts keep one per compile site (e.g. darkroom's `Engine`); the produced
/// [`CompiledGraph`] is always fresh — it's moved to the worker.
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
    ) -> Result<Compilation, CompileError> {
        // Validate before building anything: the graph+library pair is untrusted
        // input, and a passing check lets the flatten pass resolve every
        // reference infallibly.
        if let Err(e) = graph.check_for_execution(library) {
            tracing::error!(error = %e, "compile rejected: invalid graph");
            return Err(CompileError {
                message: e.to_string(),
            });
        }

        // Flatten graphs straight into execution nodes — no intermediate
        // `Graph`. Everything downstream is boundary-agnostic (func nodes only).
        let mut program = ExecutionProgram::default();
        let mut flatten_map = FlattenMap::default();
        self.flattener.build(
            &mut program.e_nodes,
            Pools {
                inputs: &mut program.inputs,
                events: &mut program.events,
                output_pinned: &mut program.output_pinned,
            },
            graph,
            library,
            &mut flatten_map,
        );

        // Resolve the program's output-type pool from the full library (every func
        // is present — `check_for_execution` validated them), making the compiled program
        // self-describing: the digest and the disk cache's codec check read it
        // with no library at run time.
        program.resolve_output_types(library);

        let flatten_map = Arc::new(flatten_map);
        let compiled = CompiledGraph {
            program,
            flatten_map: flatten_map.clone(),
        };
        compiled.check_debug(library);
        Ok(Compilation {
            compiled,
            flatten_map,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::cache::RuntimeCache;
    use crate::execution::identity::test_support::FlattenMapBuilder;
    use crate::execution::program::ExecutionNode;
    use crate::node::definition::FuncId;

    #[test]
    fn node_address_chooses_a_representative_and_preserves_unknown_ids() {
        let instance = NodeId::from_u128(1);
        let interior = NodeId::from_u128(2);
        let flat = NodeId::from_u128(3);
        let unknown = NodeId::from_u128(4);
        let mut builder = FlattenMapBuilder::new();
        builder.insert_leaf(flat, [instance], interior);
        let compiled = CompiledGraph {
            program: ExecutionProgram::default(),
            flatten_map: Arc::new(builder.build()),
        };

        assert_eq!(
            compiled.node_address(interior),
            NodeAddress {
                instances: vec![instance],
                node_id: interior,
            }
        );
        assert_eq!(compiled.node_address(unknown), NodeAddress::root(unknown));
    }

    #[test]
    fn checks_return_compiled_and_installed_mismatches() {
        let flat = NodeId::unique();
        let missing_func = FuncId::unique();
        let mut builder = FlattenMapBuilder::new();
        builder.insert_leaf(flat, [], flat);
        let mut program = ExecutionProgram::default();
        program.e_nodes.insert(
            flat,
            ExecutionNode {
                func_id: missing_func,
                ..Default::default()
            },
        );
        let compiled = CompiledGraph {
            program,
            flatten_map: Arc::new(builder.build()),
        };

        assert_eq!(
            compiled.check(&Library::default()).unwrap_err().to_string(),
            format!("execution node {flat:?} references missing func {missing_func:?}")
        );
        assert_eq!(
            compiled
                .check_installed(&RuntimeCache::default())
                .unwrap_err()
                .to_string(),
            "runtime cache node set does not match the compiled program"
        );
    }
}
