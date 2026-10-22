use std::collections::HashSet;

use super::*;
use crate::data::const_value::ConstValue;
use crate::exec::compile::error::{CompiledGraphValidationError, PortPool};
use crate::graph::identity::FuncId;
use crate::graph::node::Node;
use crate::internals;
use crate::internals::program::ProgramBuilder;
use crate::internals::test_graph::compiled::Compiled;
use crate::internals::test_graph::{NodeSpec, TestGraph};

/// The walk wires a subscription to the emitter's own event slot, and the
/// artifact carries a range backstop under it — kept because the subscriber
/// index the walk writes is the one thing a run dereferences without checking.
#[test]
fn subscription_wiring_rejects_an_endpoint_outside_the_program() {
    let mut g = TestGraph::new();
    g.add("ticker", |n| {
        n.sink().event("tick", internals::stub_event())
    });
    g.add("listener", NodeSpec::sink);
    g.subscribe("ticker", 0, "listener");

    let mut compiled = g.compile();
    assert_eq!(
        compiled.subscribers("ticker", 0),
        ["listener"],
        "the authored subscription wired one subscriber"
    );

    // The artifact check catches a subscriber index that names no node. The
    // walk cannot mint one — every index comes out of the placement, which
    // covers exactly the graph's nodes — so it is corrupted here by hand.
    let past_the_end = NodeIdx(compiled.program.e_nodes.len() as u32);
    let events = compiled.node("ticker").events;
    let subscriber = compiled.program.events[events][0].subscribers.nth(0);
    compiled.program.subscribers[subscriber] = past_the_end;
    assert!(
        matches!(
            validate::validate(&compiled.program, &g.library),
            Err(CompiledGraphValidationError::MissingEventSubscriber { subscriber, .. })
                if subscriber == past_the_end
        ),
        "a subscriber past the node vector is caught by validation"
    );
}

/// The binding-integrity backstops. The walk mints an address only from the
/// placement, so a real compile can't reach either arm — they are worth keeping
/// only if they still fire, so corrupt a compiled program's interned address two
/// ways and check both do.
#[test]
fn validation_rejects_a_binding_that_does_not_name_a_real_output() {
    let g = wired_pair(DataType::Int, DataType::Int);
    let bound_input = |compiled: &Compiled| {
        let e_node = compiled.node("consumer");
        assert!(
            matches!(compiled.binding("consumer", 0), ExecBinding::Wire(_)),
            "the fixture wires a binding to corrupt"
        );
        e_node.inputs.nth(0)
    };

    // One past the last node: the address names no node at all.
    let mut compiled = g.compile();
    let past_the_end = NodeIdx(compiled.program.e_nodes.len() as u32);
    let input = bound_input(&compiled);
    compiled.program.inputs[input].binding = ExecBinding::Wire(OutputAddr {
        node_idx: past_the_end,
        port_idx: 0,
    });
    assert!(
        matches!(
            validate::validate(&compiled.program, &g.library),
            Err(CompiledGraphValidationError::MissingBindingTarget { target, .. })
                if target.node_idx == past_the_end
        ),
        "a bind past the node vector is a missing target"
    );

    // A real node, one past its last output port.
    let mut compiled = g.compile();
    let input = bound_input(&compiled);
    let ExecBinding::Wire(address) = compiled.program.inputs[input].binding else {
        unreachable!("bound_input selected a bind")
    };
    let port_idx = compiled.program[address.node_idx].outputs.len;
    compiled.program.inputs[input].binding = ExecBinding::Wire(OutputAddr {
        node_idx: address.node_idx,
        port_idx,
    });
    assert!(
        matches!(
            validate::validate(&compiled.program, &g.library),
            Err(CompiledGraphValidationError::BindingOutputOutOfRange { target, .. })
                if target.port_idx == port_idx
        ),
        "a bind past the producer's last port is out of range"
    );

    // The override backstops: a set-aside input must be overridden from its
    // own node, and must not be wired, since no read is planned for it.
    let mut compiled = g.compile();
    let input = bound_input(&compiled);
    compiled.program.inputs[input].overridden_by = Some(input);
    assert!(
        matches!(
            validate::validate(&compiled.program, &g.library),
            Err(CompiledGraphValidationError::OverriddenBind { port_idx: 0, .. })
        ),
        "an overridden input that is wired is refused"
    );
    let outside = InputIdx(compiled.program.inputs.len() as u32);
    compiled.program.inputs[input].overridden_by = Some(outside);
    assert!(
        matches!(
            validate::validate(&compiled.program, &g.library),
            Err(CompiledGraphValidationError::OverrideOutsideNode { port_idx: 0, .. })
        ),
        "an override from past the node's inputs is refused"
    );
}

/// One sort settles the program's dense node order. Ids are uuids, so the order
/// nodes are authored in says nothing about the order they are adopted in — the
/// id column is the authored set, sorted, and nothing else.
///
/// The one fixture that picks its own ids: `TestGraph` numbers nodes in
/// declaration order, which would make "sorted" and "authored" the same list
/// and leave the sort unproven. Here they descend, so declaration order is the
/// reverse of id order.
#[test]
fn dense_order_is_id_order() {
    let mut fixture = TestGraph::new();
    fixture.add("src", |n| n.pure().output(DataType::Int));
    let func = fixture.library.by_name("src").unwrap().clone();

    let mut graph = Graph::default();
    let authored: Vec<NodeId> = (0..8u128).map(|i| NodeId::from_u128(8 - i)).collect();
    for &node_id in &authored {
        graph.insert(node_id, Node::from(&func));
    }
    let compiled = Compiler::default().compile(&graph, &fixture.library);

    let node_ids: Vec<_> = compiled.node_ids.iter().copied().collect();
    let expected: Vec<NodeId> = (1..=8u128).map(NodeId::from_u128).collect();
    assert_eq!(node_ids, expected, "nodes are adopted in id order");
    for node_id in authored {
        assert!(compiled.contains(node_id));
    }
}

#[test]
fn validation_returns_compiled_mismatches() {
    let missing_func = FuncId::unique();
    let mut prog = ProgramBuilder::default();
    let node = prog.node().func(missing_func).add();

    assert_eq!(
        validate::validate(prog.program(), &Library::default())
            .unwrap_err()
            .to_string(),
        format!(
            "execution node {:?} references missing func {missing_func:?}",
            node.node_id
        )
    );
    assert!(prog.program().contains(node.node_id));
    assert!(!prog.program().contains(NodeId::unique()));
}

/// The arity and range faults carry the pool they found rather than naming one
/// variant each, so each of the six faults has to come out a distinct
/// sentence.
#[test]
fn a_pool_fault_names_the_pool_it_found() {
    let node_id = NodeId::from_u128(1);
    let pools = [PortPool::Input, PortPool::Output, PortPool::Event];

    let arity: Vec<String> = pools
        .iter()
        .map(|&pool| CompiledGraphValidationError::Arity { node_id, pool }.to_string())
        .collect();
    let range: Vec<String> = pools
        .iter()
        .map(|&pool| CompiledGraphValidationError::Range { node_id, pool }.to_string())
        .collect();

    assert_eq!(
        arity[0],
        format!("execution node {node_id:?} input arity does not match its function")
    );
    assert_eq!(
        range[2],
        format!("execution node {node_id:?} event range is out of bounds")
    );

    let all: HashSet<&String> = arity.iter().chain(range.iter()).collect();
    assert_eq!(
        all.len(),
        6,
        "each pool and question reads distinctly: {all:?}"
    );
}

/// Everything one compile observably produced, in a deterministic order and in
/// the **stable id space** — bind targets and subscribers by id rather than
/// index, so two artifacts can't match by an index coincidence.
fn summary(compiled: &CompiledGraph, authored: &[NodeId]) -> Vec<String> {
    let mut out = Vec::new();
    for (node_idx, e_node) in compiled.e_nodes.iter_indexed() {
        let node_id = compiled.node_ids[node_idx];
        out.push(format!(
            "node {node_id:?} func={:?} sink={} disabled={} cache={:?} special={:?}",
            e_node.func_ref.id(),
            e_node.sink,
            e_node.disabled,
            e_node.cache,
            e_node.special,
        ));
        for input in &compiled.inputs[e_node.inputs] {
            let binding = match &input.binding {
                ExecBinding::None => "none".to_string(),
                ExecBinding::Const(value) => format!("const {value:?}"),
                ExecBinding::Wire(address) => format!(
                    "bind {:?}#{}",
                    compiled.node_ids[address.node_idx], address.port_idx
                ),
            };
            out.push(format!(
                "  in required={} fs_path={} {binding}",
                input.required, input.stamps_fs_path
            ));
        }
        out.push(format!("  outputs {}", e_node.outputs.len));
        for event in &compiled.events[e_node.events] {
            let subscribers: Vec<_> = compiled.subscribers[event.subscribers]
                .iter()
                .map(|&idx| compiled.node_ids[idx])
                .collect();
            out.push(format!("  event subscribers={subscribers:?}"));
        }
    }
    // The host-facing index, which is built from its own scratch.
    for &node_id in authored {
        out.push(format!(
            "authored {node_id:?} compiled={}",
            compiled.contains(node_id),
        ));
    }
    out
}

/// Every buffer a compile fills is owned by the `Compiler` and reused, so the
/// hazard the reuse introduces is one compile's leftovers reaching the next.
/// Compiling a differently shaped graph first must leave the second artifact
/// identical to what a `Compiler` that had never run produces for it.
#[test]
fn a_reused_compiler_produces_what_a_fresh_one_does() {
    // A wider graph with more ports and an event, so any pool, run length, or
    // subscriber list carried over would show up as a difference below.
    let mut warmup = TestGraph::new();
    warmup.add("emitter", |n| {
        n.pure()
            .output(DataType::String)
            .output(DataType::Float)
            .event("tick", internals::stub_event())
    });
    warmup.add("consumer", |n| {
        n.sink().input(DataType::String).input(DataType::Float)
    });
    warmup.wire("emitter", "consumer");
    warmup.wire(("emitter", 1), ("consumer", 1));
    warmup.subscribe("emitter", 0, "consumer");

    let subject = TestGraph::source_sink_loose();
    let authored: Vec<NodeId> = ["source", "sink", "loose"]
        .iter()
        .map(|name| subject.id(name))
        .collect();

    let mut reused = Compiler::default();
    reused.compile(&warmup.graph, &warmup.library);
    let after_reuse = reused.compile(&subject.graph, &subject.library);
    let fresh = Compiler::default().compile(&subject.graph, &subject.library);

    assert_eq!(
        summary(&after_reuse, &authored),
        summary(&fresh, &authored),
        "a compile carried something over from the one before it"
    );
    // The summary is only worth comparing if it saw the whole artifact.
    assert_eq!(after_reuse.e_nodes.len(), 3);
}

/// The same graph twice through one `Compiler`: the reused buffers must not
/// accumulate, which a growing pool would show as duplicated ports or nodes.
#[test]
fn recompiling_one_graph_does_not_accumulate_in_the_reused_buffers() {
    let g = TestGraph::source_sink_loose();
    let authored: Vec<NodeId> = ["source", "sink", "loose"]
        .iter()
        .map(|name| g.id(name))
        .collect();
    let mut compiler = Compiler::default();

    let first = compiler.compile(&g.graph, &g.library);
    let first_summary = summary(&first, &authored);
    let second = compiler.compile(&g.graph, &g.library);

    assert_eq!(summary(&second, &authored), first_summary);
    assert_eq!(second.inputs.len(), first.inputs.len());
    assert_eq!(second.output_count, first.output_count);
}

/// A producer declaring one output of `out_ty` and a consumer declaring one
/// required input of `in_ty`, unwired. The pair every test below is a question
/// about — each stating for itself what, if anything, reaches that input.
fn pair(out_ty: DataType, in_ty: DataType) -> TestGraph {
    let mut g = TestGraph::new();
    g.add("producer", |n| n.pure().output(out_ty));
    g.add("consumer", |n| n.sink().input(in_ty));
    g
}

/// [`pair`] with the producer wired into the consumer.
fn wired_pair(out_ty: DataType, in_ty: DataType) -> TestGraph {
    let mut g = pair(out_ty, in_ty);
    g.wire("producer", "consumer");
    g
}

/// Every authored node becomes exactly one execution node carrying its own id,
/// and the dense order is the id order — `Graph::iter` is a `HashMap` walk, so
/// the order nodes are reached in must not reach the artifact.
#[test]
fn places_one_node_per_authored_node_in_id_order() {
    let g = pair(DataType::Int, DataType::Int);
    let compiled = g.compile();

    assert_eq!(compiled.program.e_nodes.len(), 2);
    let mut expected = vec![g.id("producer"), g.id("consumer")];
    expected.sort();
    assert_eq!(
        compiled
            .program
            .node_ids
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        expected
    );
    for (pos, node_id) in expected.iter().enumerate() {
        assert_eq!(
            compiled.program.node(*node_id).unwrap(),
            NodeIdx(pos as u32)
        );
    }
}

/// A node's port runs come from its func's arity and are packed in placement
/// order, so a node owns exactly the run its own declaration claimed.
#[test]
fn packs_one_port_run_per_node_from_its_declaration() {
    let g = pair(DataType::Int, DataType::Int);
    let compiled = g.compile();

    let producer = compiled.node("producer");
    let consumer = compiled.node("consumer");
    assert_eq!((producer.outputs.len, producer.inputs.len), (1, 0));
    assert_eq!((consumer.outputs.len, consumer.inputs.len), (0, 1));
    assert_eq!(
        compiled.program.output_count, 1,
        "one output port across both nodes"
    );
    assert_eq!(compiled.program.inputs.len(), 1);
    assert_eq!(producer.outputs.start, 0, "the only run starts the pool");
}

/// A wire is interned to the producer's dense address as the walk resolves it —
/// the right node index and the port index the authored wire named.
#[test]
fn interns_a_wire_to_the_producers_dense_address() {
    let g = wired_pair(DataType::Int, DataType::Int);
    let compiled = g.compile();

    let ExecBinding::Wire(address) = *compiled.binding("consumer", 0) else {
        panic!(
            "expected an interned bind, got {:?}",
            compiled.binding("consumer", 0)
        );
    };
    assert_eq!(
        address,
        OutputAddr {
            node_idx: compiled.idx("producer"),
            port_idx: 0,
        },
    );
    // …and that address resolves into the producer's own output run, wherever
    // the id sort placed it.
    assert_eq!(
        compiled.program.output_idx(address).idx(),
        compiled.node("producer").outputs.start as usize,
    );
    assert!(
        compiled.program.inputs[compiled.node("consumer").inputs.nth(0)].required,
        "the declaration's flag travels with the port"
    );
}

/// A graph that breaks the load contract is the host's bug, so the compile
/// stops in every build, naming the binding the load or the edit gate should
/// have refused.
#[test]
#[should_panic(expected = "breaks the load contract: the binding on")]
fn a_graph_that_breaks_the_contract_panics_naming_the_breach() {
    wired_pair(DataType::String, DataType::Int).compile();
}

/// A const travels through verbatim.
#[test]
fn lowers_a_const_verbatim() {
    let mut g = pair(DataType::Int, DataType::Int);
    g.constant("consumer", 0, 7i64);
    assert!(matches!(
        g.compile().binding("consumer", 0),
        ExecBinding::Const(ConstValue::Int(7))
    ));
}

/// A disabled node keeps its place in the program — planning excludes it, the
/// walk does not — and the flag rides along for the planner to read.
#[test]
fn carries_the_disabled_flag_without_dropping_the_node() {
    let mut g = TestGraph::new();
    g.add("producer", |n| n.pure().output(DataType::Int));
    g.disable("producer");
    let compiled = g.compile();

    assert_eq!(compiled.program.e_nodes.len(), 1);
    assert!(compiled.node("producer").disabled);
}

/// Each event port gets its declared lambda and exactly the subscribers this
/// graph wires to it, so an empty list means "nothing subscribes" rather than
/// "not wired yet" — including for an emitter placed after its subscriber.
#[test]
fn wires_each_event_with_the_subscribers_resolved_for_it() {
    let mut g = TestGraph::new();
    g.add("emitter", |n| {
        n.event("quiet", internals::stub_event())
            .event("subscribed", internals::stub_event())
    });
    g.add("listener", NodeSpec::sink);
    g.subscribe("emitter", 1, "listener");

    let compiled = g.compile();
    assert_eq!(compiled.node("emitter").events.len, 2);
    assert!(
        compiled.subscribers("emitter", 0).is_empty(),
        "the unsubscribed port carries no subscriber"
    );
    assert_eq!(compiled.subscribers("emitter", 1), ["listener"]);
    assert_eq!(
        compiled.program.subscribers.len(),
        1,
        "one column holds every event's subscribers"
    );
}

/// A subscription whose emitter or subscriber is disabled wires nothing.
#[test]
fn drops_subscriptions_that_cannot_fire() {
    /// Which end of the edge, if either, the document disabled.
    #[derive(Clone, Copy, Debug)]
    enum Disabled {
        Neither,
        Emitter,
        Subscriber,
    }
    let build = |disabled: Disabled| {
        let mut g = TestGraph::new();
        g.add("emitter", |n| {
            n.event("e0", internals::stub_event())
                .event("e1", internals::stub_event())
        });
        g.add("listener", NodeSpec::sink);
        g.subscribe("emitter", 1, "listener");
        match disabled {
            Disabled::Neither => {}
            Disabled::Emitter => g.disable("emitter"),
            Disabled::Subscriber => g.disable("listener"),
        }
        let compiled = g.compile();
        (0..2)
            .map(|event_idx| compiled.subscribers("emitter", event_idx).len())
            .sum::<usize>()
    };

    assert_eq!(build(Disabled::Neither), 1, "the live subscription wires");
    assert_eq!(
        build(Disabled::Emitter),
        0,
        "a disabled emitter fires nothing"
    );
    assert_eq!(
        build(Disabled::Subscriber),
        0,
        "a disabled subscriber receives nothing"
    );
}
