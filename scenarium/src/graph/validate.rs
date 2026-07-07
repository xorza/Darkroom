//! `Graph` structural validation against a `Library`: the recursive
//! `check_with`/`check_level` pass (references resolve, no recursion, ports in
//! range, types compatible) and the debug-only `validate` asserts. The
//! library-free `check` lives on the core type in `mod.rs`.

use anyhow::{Context, ensure};
use common::{Result, is_debug};
use hashbrown::HashSet;

use super::*;
use crate::data::{DataType, StaticValue};
use crate::graph::subgraph::{SubgraphDef, SubgraphId};
use crate::library::Library;
use crate::node::function::FuncInput;

impl Graph {
    /// Debug-only internal-invariant gate (compiled out in release, so the
    /// per-edit / per-`update` callers pay nothing there). Shares its
    /// structural definition with `check`; panics because a violation here is
    /// our bug, not bad input.
    pub fn validate(&self) {
        if !is_debug() {
            return;
        }
        self.check().expect("graph structural invariant violated");
    }

    /// Debug-only assert form of [`check_with`]: a violation surfaces as a
    /// panic so a graph the editor itself built wrong is caught loudly in
    /// development. The release-safe, error-returning gate is `check_with`,
    /// which `ExecutionEngine::update` runs in every build.
    pub fn validate_with(&self, library: &Library) {
        if !is_debug() {
            return;
        }
        self.check_with(library)
            .expect("graph structural invariant violated");
    }

    /// Full structural validation against `library`, in all builds. Extends
    /// [`check`] (which can't see the library) with every library-dependent
    /// check: each func/subgraph reference resolves, no subgraph contains
    /// itself, boundary nodes sit inside a def, and every binding/subscription
    /// port index is in range. A graph+library is untrusted input at the
    /// compile boundary (a document can be stale against an evolved library),
    /// so an invalid one is a recoverable error the caller surfaces — not a
    /// panic. With this passing, flattening resolves every reference infallibly.
    pub fn check_with(&self, library: &Library) -> Result<()> {
        self.check()?;
        let mut visited: HashSet<SubgraphId> = HashSet::new();
        self.check_level(library, None, &mut visited)
    }

    /// Recursive per-level half of [`check_with`]. `ctx_def` is the enclosing
    /// subgraph definition when checking a def's interior (so boundary nodes
    /// can be checked against the interface), `None` at the top level.
    /// `visited` is the descent path of `SubgraphId`s — re-entering one is the
    /// recursion error.
    fn check_level(
        &self,
        library: &Library,
        ctx_def: Option<&SubgraphDef>,
        visited: &mut HashSet<SubgraphId>,
    ) -> Result<()> {
        // Resolve every node's func/def first (and recurse into composites):
        // the port-count helpers below look funcs/defs up infallibly, so this
        // pass must establish they all resolve before any count is taken.
        for node in self.nodes.iter() {
            match &node.kind {
                NodeKind::Func(func_id) => {
                    ensure!(
                        library.by_id(func_id).is_some(),
                        "node {:?} references func {:?}, absent from the library",
                        node.id,
                        func_id
                    );
                }
                NodeKind::Subgraph(r) => {
                    let def = self.resolve_def(*r, library).with_context(|| {
                        format!(
                            "node {:?} references a missing subgraph definition",
                            node.id
                        )
                    })?;
                    ensure!(
                        visited.insert(def.id),
                        "subgraph {:?} is recursive (contains itself)",
                        def.id
                    );
                    def.graph.check_level(library, Some(def), visited)?;
                    visited.remove(&def.id);
                }
                NodeKind::SubgraphInput => {
                    ensure!(
                        ctx_def.is_some(),
                        "SubgraphInput node is only valid inside a subgraph"
                    );
                }
                NodeKind::SubgraphOutput => {
                    ensure!(
                        ctx_def.is_some(),
                        "SubgraphOutput is only valid inside a subgraph"
                    );
                }
                // Hardcoded declaration — nothing to resolve or recurse into.
                NodeKind::Special(_) => {}
            }
        }

        // When checking a def's interior, each exposed event must name an
        // interior emitter that actually exposes that event.
        if let Some(def) = ctx_def {
            for event in &def.events {
                let emitter = self.by_id(&event.emitter).with_context(|| {
                    format!("exposed event names missing emitter {:?}", event.emitter)
                })?;
                ensure!(
                    event.emitter_event_idx < self.event_count(emitter, library, ctx_def),
                    "exposed event index {} out of range on {:?}",
                    event.emitter_event_idx,
                    event.emitter
                );
            }
        }

        // Every binding addresses ports that exist on both ends.
        for (dst, binding) in self.bindings.iter() {
            let consumer = self
                .by_id(&dst.node_id)
                .with_context(|| format!("binding on missing node {:?}", dst.node_id))?;
            ensure!(
                dst.port_idx < self.input_count(consumer, library, ctx_def),
                "binding on node {:?} input {} is out of range",
                dst.node_id,
                dst.port_idx
            );
            if let Binding::Bind(src) = binding {
                ensure!(
                    !self.input_is_const_only(consumer, dst.port_idx, library),
                    "input {} on node {:?} is const-only and cannot be wired to an upstream output",
                    dst.port_idx,
                    dst.node_id
                );
                let producer = self
                    .by_id(&src.node_id)
                    .with_context(|| format!("binding from missing node {:?}", src.node_id))?;
                ensure!(
                    src.port_idx < self.output_count(producer, library, ctx_def),
                    "binding from node {:?} output {} is out of range",
                    src.node_id,
                    src.port_idx
                );
                // Types on both ends must be compatible (`Any` is the wildcard —
                // a passthrough/reroute port). This is the engine-side boundary
                // check: with it a wired binding can't deliver the wrong type to
                // a node function, so lambdas may trust their input types. Edges
                // touching a boundary node have no concrete port type here
                // (`None`/`Any`), so they're skipped — the interface types them.
                if let Some(sink_ty) = self.input_type(library, *dst) {
                    let source_ty = self.resolve_output_type(library, *src);
                    ensure!(
                        sink_ty.compatible_with(&source_ty),
                        "node {:?} input {} expects {:?} but is wired from an incompatible {:?}",
                        dst.node_id,
                        dst.port_idx,
                        sink_ty,
                        source_ty
                    );
                }
            }
            // A `Const` literal must fit the port too, so a lambda can trust the
            // type of a constant input as much as a wired one.
            if let Binding::Const(value) = binding
                && let Some(spec) = self.input_spec(library, *dst)
            {
                ensure!(
                    const_satisfies(library, spec, value),
                    "node {:?} input {} holds a constant incompatible with its type {:?}",
                    dst.node_id,
                    dst.port_idx,
                    spec.data_type
                );
            }
        }

        // Every subscription targets an event the emitter actually exposes.
        for s in self.subscriptions.iter() {
            let emitter = self
                .by_id(&s.emitter)
                .with_context(|| format!("subscription from missing emitter {:?}", s.emitter))?;
            ensure!(
                s.event_idx < self.event_count(emitter, library, ctx_def),
                "subscription event index {} out of range on {:?}",
                s.event_idx,
                s.emitter
            );
        }
        Ok(())
    }

    /// Number of input ports a node exposes — by kind. `ctx_def` is the
    /// enclosing def, needed only for `SubgraphOutput` (whose inputs are the
    /// def's exposed outputs).
    /// Whether the consumer port `(node, port_idx)` is declared const-only — it
    /// may hold a `Const` literal but must not be wired to an upstream output.
    /// Boundary nodes route the interface (no literals), so they're never
    /// const-only. Resolution mirrors [`Self::input_count`].
    fn input_is_const_only(&self, node: &Node, port_idx: usize, library: &Library) -> bool {
        let inputs = match &node.kind {
            NodeKind::Func(func_id) => &library.by_id(func_id).unwrap().inputs,
            NodeKind::Subgraph(r) => &self.resolve_def(*r, library).unwrap().inputs,
            NodeKind::Special(s) => &s.func().inputs,
            NodeKind::SubgraphInput | NodeKind::SubgraphOutput => return false,
        };
        inputs.get(port_idx).is_some_and(|i| i.const_only)
    }

    fn input_count(&self, node: &Node, library: &Library, ctx_def: Option<&SubgraphDef>) -> usize {
        match &node.kind {
            NodeKind::Func(func_id) => library.by_id(func_id).unwrap().inputs.len(),
            NodeKind::Subgraph(r) => self.resolve_def(*r, library).unwrap().inputs.len(),
            NodeKind::Special(s) => s.func().inputs.len(),
            NodeKind::SubgraphInput => 0,
            NodeKind::SubgraphOutput => ctx_def.unwrap().outputs.len(),
        }
    }

    /// Number of output ports a node exposes — by kind. `ctx_def` is the
    /// enclosing definition, needed only for `SubgraphInput` (whose outputs
    /// are the def's exposed inputs).
    fn output_count(&self, node: &Node, library: &Library, ctx_def: Option<&SubgraphDef>) -> usize {
        match &node.kind {
            NodeKind::Func(func_id) => library.by_id(func_id).unwrap().outputs.len(),
            NodeKind::Subgraph(r) => self.resolve_def(*r, library).unwrap().outputs.len(),
            NodeKind::Special(s) => s.func().outputs.len(),
            NodeKind::SubgraphInput => ctx_def.unwrap().inputs.len(),
            NodeKind::SubgraphOutput => 0,
        }
    }

    /// Number of events a node exposes. `SubgraphInput` exposes exactly one —
    /// the trigger that interior nodes subscribe to so they fire when the
    /// enclosing composite is triggered. Infallible peer of
    /// [`Self::event_count_opt`]; callers (e.g. `check_with`) resolve every
    /// func/def first, so the lookup can't miss.
    fn event_count(&self, node: &Node, library: &Library, _ctx_def: Option<&SubgraphDef>) -> usize {
        self.event_count_opt(node, library)
            .expect("event_count on a node whose func/def is unresolved")
    }
}

/// Whether a `Const` literal `value` may sit on `input` — the `Const` half of
/// the compile-boundary type check (the `Bind` half uses
/// [`DataType::compatible_with`]). Matched directly rather than via
/// `compatible_with` because a bare `StaticValue` can't be turned back into a
/// `DataType` (it lacks the `FsPathConfig`, and the enum's variant list lives in
/// `library`).
///
/// An input carrying `value_variants` is a *pick-or-wire* port (e.g. lens's
/// preset-or-config inputs, which are `Custom`-typed for the wired case yet
/// hold an `Enum` preset literal): its constant must be exactly one of the
/// offered picks. Otherwise the literal must match the declared type — scalar
/// numerics coerce, an `Enum` literal must name a registered variant, and a
/// `Custom` port has no literal form.
fn const_satisfies(library: &Library, input: &FuncInput, value: &StaticValue) -> bool {
    if !input.value_variants.is_empty() {
        return input.value_variants.iter().any(|v| v.value == *value);
    }
    match &input.data_type {
        DataType::Any => true,
        DataType::Float | DataType::Int | DataType::Bool => matches!(
            value,
            StaticValue::Float(_) | StaticValue::Int(_) | StaticValue::Bool(_)
        ),
        DataType::String => matches!(value, StaticValue::String(_)),
        DataType::FsPath(_) => matches!(value, StaticValue::FsPath(_)),
        DataType::Enum(type_id) => matches!(
            value,
            StaticValue::Enum(name)
                if library.enum_variants(type_id).is_some_and(|vs| vs.iter().any(|v| v == name))
        ),
        DataType::Custom(_) => false,
    }
}
