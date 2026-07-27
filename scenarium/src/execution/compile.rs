//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: validate + flatten the authoring [`Graph`] against the [`Library`]
//! into a self-contained [`CompiledGraph`] the worker installs as-is. Compile
//! errors surface synchronously at the call site — a graph that doesn't
//! compile is never sent, so the worker's install is infallible and a running
//! event loop is never disturbed by a bad edit.

use hashbrown::{HashMap, HashSet};
use thiserror::Error;

use crate::execution::flatten::Flattener;
use crate::execution::identity::{ExecutionIdentityError, ExecutionNodeId, FlattenMap};
use crate::execution::program::index::{NodeIdx, NodeSet};
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::graph::{Graph, NodeId};
use crate::library::Library;

/// The graph won't compile against the library: a document can be stale
/// against an evolved library (a dropped func, a shrunk port list, a
/// type-mismatched binding), so this is a recoverable error the caller
/// surfaces, not a logic bug. The compile-phase counterpart of the run-phase
/// [`Error`](crate::execution::error::Error) — the two can't be confused at the type
/// level, and only `compile` produces it.
#[derive(Debug, Error)]
#[error("invalid graph: {message}")]
pub struct CompileError {
    pub message: String,
}

/// The compile artifact: the flattened, immutable program (lambdas, resolved
/// output types, and bound-path stamping metadata) plus the [`FlattenMap`] that
/// attributes execution identities to authored nodes. Self-contained — executing
/// it needs neither the authoring graph nor the library. `Default` is the empty
/// program (the engine's pre-install / cleared state).
#[derive(Debug, Default)]
pub struct CompiledGraph {
    pub(crate) program: ExecutionProgram,
    pub(crate) flatten_map: FlattenMap,
}

impl CompiledGraph {
    /// Return the authored leaf node that produced one execution node.
    pub fn leaf(&self, e_node_id: ExecutionNodeId) -> Result<NodeId, ExecutionIdentityError> {
        Ok(self
            .attribution(e_node_id)?
            .next()
            .expect("execution attribution must start with its authored leaf"))
    }

    /// Attribute one flat execution id to its authored node followed by every
    /// enclosing graph instance, innermost first.
    pub fn attribution(
        &self,
        e_node_id: ExecutionNodeId,
    ) -> Result<impl Iterator<Item = NodeId> + '_, ExecutionIdentityError> {
        self.flatten_map
            .attribution(e_node_id)
            .ok_or(ExecutionIdentityError::NodeNotFound { e_node_id })
    }

    /// Every execution node an authored node covers — its *footprint*.
    ///
    /// A leaf in the entry graph covers itself; a leaf inside a definition
    /// covers one occurrence per instance of that definition; a graph
    /// instance covers its whole flattened interior. The inverse of
    /// [`Self::attribution`], and the only supported way to go from an
    /// authored id to execution ids: a composite dissolves at flatten time
    /// and has no execution id of its own, so *deriving* one
    /// ([`ExecutionNodeId::from_authoring`]) answers only for a top-level
    /// leaf, while this answers for every authored node.
    ///
    /// Ascending id order, like [`Self::data_consumer_closure`].
    pub fn occurrences(&self, node_id: NodeId) -> Vec<ExecutionNodeId> {
        let program = &self.program;
        self.footprint(|covered| covered == node_id)
            .iter()
            .map(|node_idx| program.e_node_ids[node_idx])
            .collect()
    }

    /// The execution nodes a "run this node" seeds: those producing what the
    /// node exposes, plus any sink it contains.
    ///
    /// Stated without naming a node kind — an occurrence qualifies when it
    /// is a sink, or when its value leaves the footprint (something outside
    /// consumes it, or nothing does). For a leaf that is the node itself,
    /// exactly as before; for a graph instance it is the interior producers
    /// behind its output ports plus its interior sinks, and *not* the
    /// interior wiring between them — that still runs, as their upstream
    /// cone.
    ///
    /// Empty when the node has no footprint at all: a boundary node, or one
    /// absent from this program.
    pub fn run_targets(&self, node_id: NodeId) -> Vec<ExecutionNodeId> {
        let program = &self.program;
        let footprint = self.footprint(|covered| covered == node_id);
        let consumers = self.consumers();
        footprint
            .iter()
            .filter(|&node_idx| {
                program.e_nodes[node_idx].sink
                    || consumers
                        .get(&node_idx)
                        .is_none_or(|of| of.iter().any(|idx| !footprint.contains(*idx)))
            })
            .map(|node_idx| program.e_node_ids[node_idx])
            .collect()
    }

    /// Resolve authored nodes or graph instances to their flattened occurrences,
    /// then return their reflexive transitive closure over data-consumer edges.
    pub(crate) fn data_consumer_closure(
        &self,
        authored_node_ids: &[NodeId],
    ) -> Vec<ExecutionNodeId> {
        let program = &self.program;
        let selected: HashSet<NodeId> = authored_node_ids.iter().copied().collect();
        let mut in_closure = self.footprint(|covered| selected.contains(&covered));
        let mut pending: Vec<NodeIdx> = in_closure.iter().collect();
        let consumers = self.consumers();
        while let Some(node_idx) = pending.pop() {
            for &consumer_idx in consumers.get(&node_idx).into_iter().flatten() {
                if !in_closure.contains(consumer_idx) {
                    in_closure.insert(consumer_idx);
                    pending.push(consumer_idx);
                }
            }
        }

        let closure: Vec<ExecutionNodeId> = in_closure
            .iter()
            .map(|node_idx| program.e_node_ids[node_idx])
            .collect();
        debug_assert!(
            closure.is_sorted(),
            "dense indices are assigned in id order, so an ascending index walk yields ascending ids"
        );
        closure
    }

    /// Every execution node whose attribution names an authored node `covers`
    /// accepts — the shared walk behind [`Self::occurrences`],
    /// [`Self::run_targets`], and [`Self::data_consumer_closure`].
    ///
    /// Attribution yields the authored leaf followed by each enclosing
    /// instance, so testing every element is what makes a graph instance
    /// match its whole interior without the caller naming a node kind.
    fn footprint(&self, covers: impl Fn(NodeId) -> bool) -> NodeSet {
        let program = &self.program;
        let mut footprint = NodeSet::default();
        footprint.reset(program.e_nodes.len());
        for (node_idx, e_node_id) in program.e_node_ids.iter_indexed() {
            if self
                .flatten_map
                .attribution(*e_node_id)
                .expect("every execution node has authored attribution")
                .any(&covers)
            {
                footprint.insert(node_idx);
            }
        }
        footprint
    }

    /// Data-consumer edges, reversed: which nodes read each node's outputs.
    fn consumers(&self) -> HashMap<NodeIdx, Vec<NodeIdx>> {
        let program = &self.program;
        let mut consumers: HashMap<NodeIdx, Vec<NodeIdx>> = HashMap::new();
        for (node_idx, e_node) in program.e_nodes.iter_indexed() {
            for input in &program.inputs[e_node.inputs] {
                if let ExecutionBinding::Bind(address) = &input.binding {
                    consumers
                        .entry(address.node_idx)
                        .or_default()
                        .push(node_idx);
                }
            }
        }
        consumers
    }
}

/// The compile entry point, owning reusable [`Flattener`] traversal scratch.
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
    /// [installed](crate::execution::engine::ExecutionEngine::install) into an engine
    /// (typically across the worker channel).
    pub fn compile(
        &mut self,
        graph: &Graph,
        library: &Library,
    ) -> Result<CompiledGraph, CompileError> {
        // Validate before building anything: the graph+library pair is untrusted
        // input, and a passing check lets the flatten pass resolve every
        // reference infallibly.
        if let Err(e) = graph.validate_for_execution(library) {
            tracing::error!(error = %e, "compile rejected: invalid graph");
            return Err(CompileError {
                message: e.to_string(),
            });
        }

        // Flatten graphs straight into execution nodes — no intermediate
        // `Graph`. Everything downstream is boundary-agnostic (func nodes only).
        let mut program = ExecutionProgram::default();
        let mut flatten_map = FlattenMap::default();
        self.flattener
            .build(&mut program, graph, library, &mut flatten_map);

        // Resolve types here so runtime digesting does not retain the function library.
        program.resolve_output_types(library);

        let compiled = CompiledGraph {
            program,
            flatten_map,
        };
        compiled.validate_debug(library);
        Ok(compiled)
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use std::sync::Arc;

    use crate::execution::compile::CompiledGraph;
    use crate::execution::identity::{ExecutionNodeId, FlattenMap};
    use crate::execution::program::ExecutionProgram;
    use crate::graph::NodeId;

    #[derive(Debug)]
    pub struct CompiledGraphBuilder {
        flatten_map: FlattenMap,
    }

    impl CompiledGraphBuilder {
        pub fn new() -> Self {
            let mut flatten_map = FlattenMap::default();
            flatten_map.reset();
            Self { flatten_map }
        }

        pub fn insert_leaf(
            &mut self,
            e_node_id: ExecutionNodeId,
            instances: impl IntoIterator<Item = NodeId>,
            node_id: NodeId,
        ) {
            let mut scope = 0;
            for instance in instances {
                scope = self.flatten_map.push_scope(instance, scope);
            }
            self.flatten_map.set_leaf(e_node_id, scope, node_id);
        }

        pub fn build(self) -> Arc<CompiledGraph> {
            Arc::new(CompiledGraph {
                program: ExecutionProgram::default(),
                flatten_map: self.flatten_map,
            })
        }
    }

    impl Default for CompiledGraphBuilder {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::cache::runtime::RuntimeCache;
    use crate::execution::cache::slot::{RuntimeSlot, StateOwner};
    use crate::execution::identity::ExecutionEventPort;
    use crate::execution::identity::internals::FlattenMapBuilder;
    use crate::execution::program::index::OutputAddr;
    use crate::execution::program::{ExecutionNode, PendingSubscription};
    use crate::execution::validate::CompiledGraphValidationError;
    use crate::graph::interface::{GraphId, GraphLink};
    use crate::graph::{GraphDef, NodeSearch};
    use crate::node::definition::{Func, FuncId};
    use crate::node::event::EventLambda;
    use crate::testing::{self, TestFuncHooks, test_func_lib, test_graph};

    /// Event edges get the same treatment as bind fixups: an endpoint flatten
    /// never emitted is a flatten bug, so wiring panics instead of dropping the
    /// edge, and the compiled artifact still carries a range backstop.
    #[test]
    fn subscription_wiring_rejects_an_endpoint_outside_the_program() {
        let mut library = test_func_lib(TestFuncHooks::default());
        library.add(testing::with_stub_lambda(
            Func::new(FuncId::unique(), "ticker")
                .category("Test")
                .sink()
                .event("tick", EventLambda::default()),
        ));
        let mut graph = Graph::default();
        let emitter = graph.add(library.by_name("ticker").unwrap().into());
        let subscriber = graph.add(library.by_name("Print").unwrap().into());
        graph.subscribe(emitter, 0, subscriber);

        let mut compiled = Compiler::default().compile(&graph, &library).unwrap();
        let emitter_idx =
            compiled.program.e_node_index[&ExecutionNodeId::from_authoring(&[emitter])];
        let events = compiled.program[emitter_idx].events;
        assert_eq!(
            compiled.program.events[events][0].subscribers.len(),
            1,
            "the authored subscription wired one flat subscriber"
        );

        // An unemitted subscriber is a flatten bug, not drift to absorb.
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compiled.program.apply_subscriptions(&[PendingSubscription {
                event: ExecutionEventPort {
                    e_node_id: ExecutionNodeId::from_authoring(&[emitter]),
                    event_idx: 0,
                },
                subscriber: ExecutionNodeId::unique(),
            }]);
        }));
        assert!(
            panic.is_err(),
            "wiring a subscriber the program never adopted must panic"
        );

        // And the artifact check catches a subscriber index that names no node.
        let past_the_end = NodeIdx(compiled.program.e_nodes.len() as u32);
        compiled.program.events[events][0].subscribers[0] = past_the_end;
        assert!(
            matches!(
                compiled.validate(&library),
                Err(CompiledGraphValidationError::MissingEventSubscriber { subscriber, .. })
                    if subscriber == past_the_end
            ),
            "a subscriber past the node vector is caught by validation"
        );
    }

    /// The binding-integrity backstops. `intern_bindings` mints an address only
    /// from a successful id lookup and a real compile can't reach either arm, so
    /// they are only worth keeping if they still fire — corrupt a compiled
    /// program's interned address two ways and check both do.
    #[test]
    fn validation_rejects_a_binding_that_does_not_name_a_real_output() {
        let library = test_func_lib(TestFuncHooks::default());
        let compile = || {
            Compiler::default()
                .compile(&test_graph(), &library)
                .unwrap()
        };
        let bound_input = |compiled: &CompiledGraph| {
            (0..compiled.program.inputs.len())
                .find(|i| {
                    matches!(
                        compiled.program.inputs[*i].binding,
                        ExecutionBinding::Bind(_)
                    )
                })
                .expect("the test graph wires bindings")
        };

        // One past the last node: the address names no node at all.
        let mut compiled = compile();
        let past_the_end = NodeIdx(compiled.program.e_nodes.len() as u32);
        let input = bound_input(&compiled);
        compiled.program.inputs[input].binding = ExecutionBinding::Bind(OutputAddr {
            node_idx: past_the_end,
            port_idx: 0,
        });
        assert!(
            matches!(
                compiled.validate(&library),
                Err(CompiledGraphValidationError::MissingBindingTarget { target, .. })
                    if target.node_idx == past_the_end
            ),
            "a bind past the node vector is a missing target"
        );

        // A real node, one past its last output port.
        let mut compiled = compile();
        let input = bound_input(&compiled);
        let ExecutionBinding::Bind(address) = compiled.program.inputs[input].binding else {
            unreachable!("bound_input selected a bind")
        };
        let port_idx = compiled.program[address.node_idx].outputs.len;
        compiled.program.inputs[input].binding = ExecutionBinding::Bind(OutputAddr {
            node_idx: address.node_idx,
            port_idx,
        });
        assert!(
            matches!(
                compiled.validate(&library),
                Err(CompiledGraphValidationError::BindingOutputOutOfRange { target, .. })
                    if target.port_idx == port_idx
            ),
            "a bind past the producer's last port is out of range"
        );
    }

    #[test]
    fn compilation_retains_a_disabled_composite_interior_as_disabled() {
        let library = test_func_lib(TestFuncHooks::default());
        let mut nested = GraphDef::new("Nested");
        let interior_id = nested.body.add(library.by_name("Print").unwrap().into());
        let nested_id = GraphId::unique();

        let mut graph = Graph::default();
        let instance_id = graph.add_graph_node(&nested, GraphLink::Local(nested_id));
        graph
            .find_mut(instance_id, NodeSearch::TopLevel)
            .unwrap()
            .disabled = true;
        graph.insert_graph(nested_id, nested);

        let compiled = Compiler::default().compile(&graph, &library).unwrap();
        let e_node_id = ExecutionNodeId::from_authoring(&[instance_id, interior_id]);
        assert!(
            compiled.program.by_id(e_node_id).disabled,
            "the disabled instance marks its compiled interior effectively disabled"
        );
    }

    #[test]
    fn data_consumer_closure_targets_one_instance_or_every_shared_definition_occurrence() {
        let library = test_func_lib(TestFuncHooks::default());
        let mut nested = GraphDef::new("Nested");
        let interior_id = nested.body.add(library.by_name("get_b").unwrap().into());
        let nested_id = GraphId::unique();

        let mut graph = Graph::default();
        let first_instance = graph.add_graph_node(&nested, GraphLink::Local(nested_id));
        let second_instance = graph.add_graph_node(&nested, GraphLink::Local(nested_id));
        graph.insert_graph(nested_id, nested);
        let first = ExecutionNodeId::from_authoring(&[first_instance, interior_id]);
        let second = ExecutionNodeId::from_authoring(&[second_instance, interior_id]);
        let compiled = Compiler::default().compile(&graph, &library).unwrap();

        assert_eq!(
            compiled.data_consumer_closure(&[first_instance]),
            vec![first],
            "an instance evicts only its own flattened interior"
        );
        let mut both = vec![first, second];
        both.sort_unstable();
        assert_eq!(
            compiled.data_consumer_closure(&[interior_id]),
            both,
            "editing a shared definition evicts every flattened occurrence"
        );
    }

    /// A composite whose interior distinguishes all three run-target cases:
    /// `relay` backs the instance's one output, `printer` is an interior
    /// sink, and `source` feeds only the other two.
    struct NestedFixture {
        library: Library,
        graph: Graph,
        instance: NodeId,
        boundary: NodeId,
        source: NodeId,
        relay: NodeId,
        printer: NodeId,
        consumer: NodeId,
    }

    fn nested_fixture() -> NestedFixture {
        use crate::data::type_system::DataType;
        use crate::graph::{Binding, InputPort, Node, NodeKind};
        use crate::node::definition::FuncOutput;

        let library = test_func_lib(TestFuncHooks::default());
        let mut nested = GraphDef::new("Nested").output(FuncOutput::new("out", DataType::Int));
        let boundary = nested.body.add(Node::new(NodeKind::GraphOutput));
        let source = nested.body.add(library.by_name("get_b").unwrap().into());
        let relay = nested.body.add(library.by_name("sum").unwrap().into());
        let printer = nested.body.add(library.by_name("Print").unwrap().into());
        nested
            .body
            .set_input_binding(InputPort::new(relay, 0), Binding::bind(source, 0));
        nested
            .body
            .set_input_binding(InputPort::new(printer, 0), Binding::bind(source, 0));
        nested
            .body
            .set_input_binding(InputPort::new(boundary, 0), Binding::bind(relay, 0));

        let nested_id = GraphId::unique();
        let mut graph = Graph::default();
        let instance = graph.add_graph_node(&nested, GraphLink::Local(nested_id));
        let consumer = graph.add(library.by_name("Print").unwrap().into());
        graph.set_input_binding(InputPort::new(consumer, 0), Binding::bind(instance, 0));
        graph.insert_graph(nested_id, nested);

        NestedFixture {
            library,
            graph,
            instance,
            boundary,
            source,
            relay,
            printer,
            consumer,
        }
    }

    #[test]
    fn an_authored_nodes_footprint_covers_a_leaf_or_a_whole_interior() {
        let f = nested_fixture();
        let compiled = Compiler::default().compile(&f.graph, &f.library).unwrap();
        let interior = |node_id| ExecutionNodeId::from_authoring(&[f.instance, node_id]);

        // A leaf in the entry graph covers exactly itself, and its execution
        // id is the authored one.
        assert_eq!(
            compiled.occurrences(f.consumer),
            vec![ExecutionNodeId::from_authoring(&[f.consumer])]
        );
        // A leaf inside the definition covers its one occurrence.
        assert_eq!(compiled.occurrences(f.relay), vec![interior(f.relay)]);

        // The instance covers its whole interior — and nothing outside it.
        let mut whole_interior = vec![interior(f.source), interior(f.relay), interior(f.printer)];
        whole_interior.sort_unstable();
        assert_eq!(compiled.occurrences(f.instance), whole_interior);

        // Boundary nodes are wiring: flatten resolves through them and emits
        // nothing, so they cover no execution work at all. Same for a node
        // this program never saw.
        assert!(compiled.occurrences(f.boundary).is_empty());
        assert!(compiled.occurrences(NodeId::unique()).is_empty());
    }

    #[test]
    fn run_targets_seed_what_a_node_exposes_plus_the_sinks_it_contains() {
        let f = nested_fixture();
        let compiled = Compiler::default().compile(&f.graph, &f.library).unwrap();
        let interior = |node_id| ExecutionNodeId::from_authoring(&[f.instance, node_id]);

        // A leaf seeds itself, whether its value leaves (`consumer` reads the
        // instance) or it is a sink with no value at all.
        assert_eq!(
            compiled.run_targets(f.consumer),
            vec![ExecutionNodeId::from_authoring(&[f.consumer])]
        );
        assert_eq!(compiled.run_targets(f.relay), vec![interior(f.relay)]);
        assert_eq!(compiled.run_targets(f.source), vec![interior(f.source)]);

        // The instance seeds the producer behind its output port plus its
        // interior sink — but not `source`, whose value never leaves the
        // footprint. It still runs, as their upstream cone.
        let mut exposed = vec![interior(f.relay), interior(f.printer)];
        exposed.sort_unstable();
        assert_eq!(compiled.run_targets(f.instance), exposed);
        assert!(
            !compiled
                .run_targets(f.instance)
                .contains(&interior(f.source)),
            "a purely interior producer is a dependency, not a target"
        );

        // Nothing to seed for a node with no footprint.
        assert!(compiled.run_targets(f.boundary).is_empty());
    }

    #[test]
    fn validation_returns_compiled_and_installed_mismatches() {
        let e_node_id = ExecutionNodeId::unique();
        let interior = NodeId::unique();
        let missing_func = FuncId::unique();
        let mut builder = FlattenMapBuilder::new();
        builder.insert_leaf(e_node_id, [], interior);
        let mut program = ExecutionProgram::default();
        program.push(
            e_node_id,
            ExecutionNode {
                func_id: missing_func,
                ..Default::default()
            },
        );
        let compiled = CompiledGraph {
            program,
            flatten_map: builder.build(),
        };

        assert_eq!(
            compiled
                .validate(&Library::default())
                .unwrap_err()
                .to_string(),
            format!("execution node {e_node_id:?} references missing func {missing_func:?}")
        );
        assert_eq!(
            compiled
                .validate_installed(&RuntimeCache::default())
                .unwrap_err()
                .to_string(),
            "runtime cache node set does not match the compiled program"
        );

        // Right slot count, wrong node: only the cache's own id column can
        // catch this — the state owners match, so the owner compare stays quiet.
        let stranger = ExecutionNodeId::unique();
        let mut misaligned = RuntimeCache::default();
        misaligned.e_node_ids.push(stranger);
        misaligned.slots.push(RuntimeSlot {
            owner: StateOwner {
                func_id: missing_func,
                version: 0,
            },
            ..Default::default()
        });
        assert_eq!(
            compiled
                .validate_installed(&misaligned)
                .unwrap_err()
                .to_string(),
            format!(
                "runtime cache slot {:?} holds node {stranger:?}, not {e_node_id:?}",
                NodeIdx(0)
            )
        );

        assert_eq!(
            compiled.attribution(e_node_id).unwrap().collect::<Vec<_>>(),
            vec![interior]
        );
        assert_eq!(compiled.leaf(e_node_id).unwrap(), interior);

        let missing_node = ExecutionNodeId::unique();
        assert_eq!(
            compiled.leaf(missing_node),
            Err(ExecutionIdentityError::NodeNotFound {
                e_node_id: missing_node,
            })
        );
        assert!(matches!(
            compiled.attribution(missing_node),
            Err(ExecutionIdentityError::NodeNotFound { e_node_id }) if e_node_id == missing_node
        ));
    }
}
