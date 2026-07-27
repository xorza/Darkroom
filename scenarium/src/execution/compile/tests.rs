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
    let emitter_idx = compiled.program.e_node_index[&ExecutionNodeId::from_authoring(&[emitter])];
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
    nested_id: GraphId,
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
        nested_id,
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
fn per_node_facts_fold_over_a_footprint_rather_than_a_composites_own_shape() {
    let f = nested_fixture();
    let compiled = Compiler::default().compile(&f.graph, &f.library).unwrap();

    // A func answers from its own declaration. `Print` is a sink and, like
    // every func that doesn't declare `.pure()`, impure; `sum` is neither.
    assert_eq!(compiled.is_sink(f.consumer), Some(true));
    assert_eq!(compiled.is_impure(f.consumer), Some(true));
    assert_eq!(compiled.is_sink(f.relay), Some(false));
    assert_eq!(compiled.is_impure(f.relay), Some(false));

    // The instance exposes an output *and* wraps `Print`. Nothing about its
    // own shape says either fact: port arity says "not a sink", and it has no
    // declaration at all to be impure by. Its interior says both — and the
    // interior is what a sinks run reaches, what disabling it suppresses, and
    // what stops its result being reusable.
    assert!(
        !f.graph
            .find_graph(f.nested_id)
            .unwrap()
            .interface
            .outputs
            .is_empty(),
        "the fixture instance exposes an output, or this proves nothing"
    );
    assert_eq!(compiled.is_sink(f.instance), Some(true));
    assert_eq!(compiled.is_impure(f.instance), Some(true));

    // Nothing to fold: boundary nodes emit no work, and neither does a node
    // this program never saw. The caller keeps its own reading.
    assert_eq!(compiled.is_sink(f.boundary), None);
    assert_eq!(compiled.is_impure(f.boundary), None);
    assert_eq!(compiled.is_sink(NodeId::unique()), None);
    assert_eq!(compiled.is_impure(NodeId::unique()), None);

    // A composite wrapping neither is neither — the fold reports what is
    // there, it doesn't assume composites are special.
    let plain = {
        use crate::data::type_system::DataType;
        use crate::graph::{Binding, InputPort, Node, NodeKind};
        use crate::node::definition::FuncOutput;

        let mut nested = GraphDef::new("Plain").output(FuncOutput::new("out", DataType::Int));
        let boundary = nested.body.add(Node::new(NodeKind::GraphOutput));
        let source = nested.body.add(f.library.by_name("get_b").unwrap().into());
        nested
            .body
            .set_input_binding(InputPort::new(boundary, 0), Binding::bind(source, 0));
        let nested_id = GraphId::unique();
        let mut graph = Graph::default();
        let instance = graph.add_graph_node(&nested, GraphLink::Local(nested_id));
        graph.insert_graph(nested_id, nested);
        let compiled = Compiler::default().compile(&graph, &f.library).unwrap();
        (compiled.is_sink(instance), compiled.is_impure(instance))
    };
    assert_eq!(plain, (Some(false), Some(false)));
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
    let compiled = CompiledGraph::indexed(program, builder.build());

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

    let missing_node = ExecutionNodeId::unique();
    assert!(matches!(
        compiled.attribution(missing_node),
        Err(ExecutionIdentityError::NodeNotFound { e_node_id }) if e_node_id == missing_node
    ));
}
