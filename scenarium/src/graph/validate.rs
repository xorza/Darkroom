//! `Graph` structural validation against a `Library`: the recursive
//! `check_with`/`check_level` pass (references resolve, no recursion, ports in
//! range, types compatible) and the debug-only `validate` asserts. The
//! library-free `check` lives on the core type in `mod.rs`.

use anyhow::{Context, ensure};
use common::{Result, is_debug};
use hashbrown::HashSet;

use crate::graph::{Binding, Graph, Node, NodeKind, NodeSearch};
use crate::library::Library;
use crate::node::definition::FuncInput;
use crate::{DataType, StaticValue};

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
    /// check: each func/graph reference resolves, no graph contains itself,
    /// and every binding/subscription
    /// port index is in range. A graph+library is untrusted input at the
    /// compile boundary (a document can be stale against an evolved library),
    /// so an invalid one is a recoverable error the caller surfaces — not a
    /// panic. With this passing, flattening resolves every reference infallibly.
    pub fn check_with(&self, library: &Library) -> Result<()> {
        self.check()?;
        ensure!(
            self.inputs.is_empty() && self.outputs.is_empty() && self.events.is_empty(),
            "entry graph cannot expose an interface"
        );
        ensure!(
            self.nodes.values().all(|node| !node.kind.is_boundary()),
            "entry graph cannot contain interface boundary nodes"
        );
        self.check_unique_node_ids(Some(library))?;
        let mut visited = HashSet::new();
        visited.insert(std::ptr::from_ref(self));
        self.check_level(library, &mut visited)
    }

    /// Recursive per-level half of [`check_with`].
    fn check_level(&self, library: &Library, visited: &mut HashSet<*const Graph>) -> Result<()> {
        // Resolve every node's func/graph first (and recurse into composites):
        // the port-count helpers below look them up infallibly, so this
        // pass must establish they all resolve before any count is taken.
        for (node_id, node) in &self.nodes {
            match &node.kind {
                NodeKind::Func(func_id) => {
                    ensure!(
                        library.by_id(func_id).is_some(),
                        "node {:?} references func {:?}, absent from the library",
                        node_id,
                        func_id
                    );
                }
                NodeKind::Graph(link) => {
                    let graph = self.resolve_graph(*link, library).with_context(|| {
                        format!("node {:?} references a missing graph", node_id)
                    })?;
                    let graph_ptr = std::ptr::from_ref(graph);
                    ensure!(
                        visited.insert(graph_ptr),
                        "graph {:?} is recursive (contains itself)",
                        graph.name
                    );
                    graph.check_level(library, visited)?;
                    visited.remove(&graph_ptr);
                }
                NodeKind::GraphInput | NodeKind::GraphOutput => {}
                // Hardcoded declaration — nothing to resolve or recurse into.
                NodeKind::Special(_) => {}
            }
        }

        for event in &self.events {
            let emitter = self
                .find(&event.emitter, NodeSearch::TopLevel)
                .with_context(|| {
                    format!("exposed event names missing emitter {:?}", event.emitter)
                })?;
            ensure!(
                event.emitter_event_idx < self.event_count(emitter, library),
                "exposed event index {} out of range on {:?}",
                event.emitter_event_idx,
                event.emitter
            );
        }

        // Every binding addresses ports that exist on both ends.
        for (dst, binding) in self.bindings.iter() {
            let consumer = self
                .find(&dst.node_id, NodeSearch::TopLevel)
                .with_context(|| format!("binding on missing node {:?}", dst.node_id))?;
            ensure!(
                dst.port_idx < self.input_count(consumer, library),
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
                    .find(&src.node_id, NodeSearch::TopLevel)
                    .with_context(|| format!("binding from missing node {:?}", src.node_id))?;
                ensure!(
                    src.port_idx < self.output_count(producer, library),
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
                .find(&s.emitter, NodeSearch::TopLevel)
                .with_context(|| format!("subscription from missing emitter {:?}", s.emitter))?;
            ensure!(
                s.event_idx < self.event_count(emitter, library),
                "subscription event index {} out of range on {:?}",
                s.event_idx,
                s.emitter
            );
        }

        // Every pinned output addresses a port that actually exists.
        for port in self.pinned_outputs.iter() {
            let node = self
                .find(&port.node_id, NodeSearch::TopLevel)
                .with_context(|| format!("pinned output on missing node {:?}", port.node_id))?;
            ensure!(
                port.port_idx < self.output_count(node, library),
                "pinned output on node {:?} output {} is out of range",
                port.node_id,
                port.port_idx
            );
        }
        Ok(())
    }

    /// Whether the consumer port `(node, port_idx)` is declared const-only — it
    /// may hold a `Const` literal but must not be wired to an upstream output.
    /// Boundary nodes route the interface (no literals), so they're never
    /// const-only. Resolution mirrors [`Self::input_count`].
    fn input_is_const_only(&self, node: &Node, port_idx: usize, library: &Library) -> bool {
        let inputs = match &node.kind {
            NodeKind::Func(func_id) => &library.by_id(func_id).unwrap().inputs,
            NodeKind::Graph(r) => &self.resolve_graph(*r, library).unwrap().inputs,
            NodeKind::Special(s) => &s.func().inputs,
            NodeKind::GraphInput | NodeKind::GraphOutput => return false,
        };
        inputs.get(port_idx).is_some_and(|i| i.const_only)
    }

    fn input_count(&self, node: &Node, library: &Library) -> usize {
        match &node.kind {
            NodeKind::Func(func_id) => library.by_id(func_id).unwrap().inputs.len(),
            NodeKind::Graph(r) => self.resolve_graph(*r, library).unwrap().inputs.len(),
            NodeKind::Special(s) => s.func().inputs.len(),
            NodeKind::GraphInput => 0,
            NodeKind::GraphOutput => self.outputs.len(),
        }
    }

    fn output_count(&self, node: &Node, library: &Library) -> usize {
        match &node.kind {
            NodeKind::Func(func_id) => library.by_id(func_id).unwrap().outputs.len(),
            NodeKind::Graph(r) => self.resolve_graph(*r, library).unwrap().outputs.len(),
            NodeKind::Special(s) => s.func().outputs.len(),
            NodeKind::GraphInput => self.inputs.len(),
            NodeKind::GraphOutput => 0,
        }
    }

    /// Number of events a node exposes. `GraphInput` exposes exactly one —
    /// the trigger that interior nodes subscribe to so they fire when the
    /// enclosing composite is triggered. Infallible peer of
    /// [`Self::event_count_opt`]; callers (e.g. `check_with`) resolve every
    /// func or graph first, so the lookup can't miss.
    fn event_count(&self, node: &Node, library: &Library) -> usize {
        self.event_count_opt(node, library)
            .expect("event_count on a node whose func or graph is unresolved")
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
