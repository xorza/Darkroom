//! Self-consistency checks for the compiled program, runtime cache, and per-run
//! plan. Each fallible `check` method has an `is_debug()`-gated `check_debug`
//! wrapper, so production call sites pay nothing while tests can inspect exact
//! validation errors.

use anyhow::{Context, Result, ensure};
use common::is_debug;
use hashbrown::HashSet;

use crate::execution::cache::RuntimeCache;
use crate::execution::compile::CompiledGraph;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::library::Library;

impl CompiledGraph {
    /// Self-consistency of the freshly compiled artifact against the `Library`
    /// it was compiled from. The source graph is gone after flattening, so this
    /// validates each `e_node` against its func and checks binding integrity.
    /// The debug wrapper runs at compile (where the library is in hand); the
    /// library-free install-side checks live in [`Self::check_installed`].
    pub(crate) fn check(&self, library: &Library) -> Result<()> {
        let program = &self.program;
        self.flatten_map.check(program.e_nodes.keys().copied())?;
        for (node_id, e_node) in &program.e_nodes {
            ensure!(
                !e_node.func_id.is_nil(),
                "execution node {node_id:?} has a nil func id"
            );
            // A special node's interface is its hardcoded spec, not a library func.
            let func = match e_node.special {
                Some(special) => special.func(),
                None => library.by_id(&e_node.func_id).with_context(|| {
                    format!(
                        "execution node {node_id:?} references missing func {:?}",
                        e_node.func_id
                    )
                })?,
            };
            ensure!(
                e_node.inputs.len as usize == func.inputs.len(),
                "execution node {node_id:?} input arity does not match its function"
            );
            ensure!(
                e_node.outputs.len as usize == func.outputs.len(),
                "execution node {node_id:?} output arity does not match its function"
            );
            ensure!(
                e_node.events.len as usize == func.events.len(),
                "execution node {node_id:?} event arity does not match its function"
            );
            let inputs = program.inputs.get(e_node.inputs.range()).with_context(|| {
                format!("execution node {node_id:?} input span is out of range")
            })?;
            ensure!(
                program.output_types.get(e_node.outputs.range()).is_some(),
                "execution node {node_id:?} output span is out of range"
            );
            ensure!(
                program.output_pinned.get(e_node.outputs.range()).is_some(),
                "execution node {node_id:?} pinned-output span is out of range"
            );
            ensure!(
                program.events.get(e_node.events.range()).is_some(),
                "execution node {node_id:?} event span is out of range"
            );

            for e_input in inputs {
                if let ExecutionBinding::Bind(e_addr) = &e_input.binding {
                    let target = program.e_nodes.get(&e_addr.target).with_context(|| {
                        format!(
                            "execution node {node_id:?} binds to missing node {:?}",
                            e_addr.target
                        )
                    })?;
                    ensure!(
                        e_addr.port_idx < target.outputs.len as usize,
                        "execution node {node_id:?} binds to out-of-range output {} on {:?}",
                        e_addr.port_idx,
                        e_addr.target
                    );
                }
            }
        }
        Ok(())
    }

    /// Debug-only assert form of [`Self::check`].
    pub(crate) fn check_debug(&self, library: &Library) {
        if !is_debug() {
            return;
        }
        self.check(library)
            .expect("compiled graph invariant violated");
    }

    /// The engine's runtime `cache` has exactly this artifact's node ids after
    /// `reconcile` — the install-side half of the checks;
    /// artifact-vs-library consistency runs at compile ([`Self::check`]).
    pub(crate) fn check_installed(&self, cache: &RuntimeCache) -> Result<()> {
        ensure!(
            cache.slots.len() == self.program.e_nodes.len(),
            "runtime cache node set does not match the compiled program"
        );

        for (node_id, e_node) in &self.program.e_nodes {
            let slot = cache
                .slots
                .get(node_id)
                .with_context(|| format!("runtime cache is missing node {node_id:?}"))?;
            if let Some(output_values) = slot.output_values() {
                ensure!(
                    output_values.len() == e_node.outputs.len as usize,
                    "runtime cache output arity does not match node {node_id:?}"
                );
            }
        }
        Ok(())
    }

    /// Debug-only assert form of [`Self::check_installed`].
    pub(crate) fn check_installed_debug(&self, cache: &RuntimeCache) {
        if !is_debug() {
            return;
        }
        self.check_installed(cache)
            .expect("installed compiled graph invariant violated");
    }
}

impl ExecutionPlan {
    /// A planned schedule is a unique post-order DFS whose bindings name valid outputs.
    pub(crate) fn check(&self, program: &ExecutionProgram) -> Result<()> {
        ensure!(
            self.process_order.len() <= program.e_nodes.len(),
            "execution order contains more entries than the program"
        );

        let mut seen_in_order = HashSet::with_capacity(program.e_nodes.len());
        for &node_id in &self.process_order {
            let e_node = program
                .e_nodes
                .get(&node_id)
                .with_context(|| format!("execution order contains missing node {node_id:?}"))?;
            let inputs = program.inputs.get(e_node.inputs.range()).with_context(|| {
                format!("execution node {node_id:?} input span is out of range")
            })?;
            for input in inputs {
                if let ExecutionBinding::Bind(addr) = &input.binding {
                    ensure!(
                        seen_in_order.contains(&addr.target),
                        "execution node {node_id:?} appears before dependency {:?}",
                        addr.target
                    );
                }
            }
            ensure!(
                seen_in_order.insert(node_id),
                "execution order contains duplicate node {node_id:?}"
            );
        }

        for node_id in &self.pinned {
            ensure!(
                self.roots.contains(node_id),
                "pinned node {node_id:?} is not an execution root"
            );
        }

        for (node_id, e_node) in &program.e_nodes {
            let inputs = program.inputs.get(e_node.inputs.range()).with_context(|| {
                format!("execution node {node_id:?} input span is out of range")
            })?;
            for e_input in inputs {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    let target = program.e_nodes.get(&addr.target).with_context(|| {
                        format!(
                            "execution node {node_id:?} binds to missing node {:?}",
                            addr.target
                        )
                    })?;
                    ensure!(
                        addr.port_idx < target.outputs.len as usize,
                        "execution node {node_id:?} binds to out-of-range output {} on {:?}",
                        addr.port_idx,
                        addr.target
                    );
                }
            }
        }
        Ok(())
    }

    /// Debug-only assert form of [`Self::check`].
    pub(crate) fn check_debug(&self, program: &ExecutionProgram) {
        if !is_debug() {
            return;
        }
        self.check(program)
            .expect("execution plan invariant violated");
    }
}
