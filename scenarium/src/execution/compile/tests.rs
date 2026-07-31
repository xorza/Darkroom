use std::collections::HashSet;

use super::*;
use crate::common::column::Idx;
use crate::execution::compile::compiled_graph::{CompiledGraph, ExecutionBinding};
use crate::execution::compile::error::{CompiledGraphValidationError, PortPool};
use crate::execution::identity::NodeIdx;
use crate::execution::identity::OutputAddr;
use crate::graph::func::event::EventLambda;
use crate::graph::identity::{FuncId, InputPort, NodeId, OutputPort};
use crate::graph::node::Node;
use crate::graph::output_types::OutputTypes;
use crate::testing::graph::TestGraph;
use crate::testing::graph::compiled::Compiled;
use crate::testing::program::ProgramBuilder;
use crate::{DataType, StaticValue};

/// The walk wires a subscription to the emitter's own event slot, and the
/// artifact carries a range backstop under it — kept because the subscriber
/// index the walk writes is the one thing a run dereferences without checking.
#[test]
fn subscription_wiring_rejects_an_endpoint_outside_the_program() {
    let mut g = TestGraph::new();
    g.add("ticker", |n| n.sink().event("tick", EventLambda::default()));
    g.add("listener", |n| n.sink());
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
    compiled.program.events[events][0].subscribers[0] = past_the_end;
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
            matches!(compiled.binding("consumer", 0), ExecutionBinding::Bind(_)),
            "the fixture wires a binding to corrupt"
        );
        e_node.inputs.nth(0)
    };

    // One past the last node: the address names no node at all.
    let mut compiled = g.compile();
    let past_the_end = NodeIdx(compiled.program.e_nodes.len() as u32);
    let input = bound_input(&compiled);
    compiled.program.inputs[input].binding = ExecutionBinding::Bind(OutputAddr {
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
            validate::validate(&compiled.program, &g.library),
            Err(CompiledGraphValidationError::BindingOutputOutOfRange { target, .. })
                if target.port_idx == port_idx
        ),
        "a bind past the producer's last port is out of range"
    );
}

/// One sort settles the program's dense node order. Ids are uuids, so the order
/// nodes are authored in says nothing about the order they are adopted in — the
/// id column is the authored set, sorted, and nothing else.
///
/// The one fixture that mints its own ids: `TestGraph` numbers nodes in
/// declaration order, which would make "sorted" and "authored" the same list
/// and leave the sort unproven.
#[test]
fn dense_order_is_id_order() {
    let mut fixture = TestGraph::new();
    fixture.add("src", |n| n.pure().output(DataType::Int));
    let func = fixture.library.by_name("src").unwrap().clone();

    let mut graph = Graph::default();
    let authored: Vec<NodeId> = (0..8).map(|_| graph.add(Node::from(&func))).collect();
    let compiled = Compiler::default()
        .compile(&graph, &fixture.library)
        .unwrap();

    let node_ids: Vec<_> = compiled.node_ids.iter().copied().collect();
    let mut expected = authored.clone();
    expected.sort();
    assert_eq!(node_ids, expected, "nodes are adopted in id order");
    assert_ne!(
        node_ids, authored,
        "eight random uuids do not land in declaration order, so the sort is the reason"
    );
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
/// variant each, so the six messages that used to be six variants have to still
/// come out six distinct sentences.
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
            e_node.func_id, e_node.sink, e_node.disabled, e_node.cache, e_node.special,
        ));
        for input in &compiled.inputs[e_node.inputs] {
            let binding = match &input.binding {
                ExecutionBinding::None => "none".to_string(),
                ExecutionBinding::Const(value) => format!("const {value:?}"),
                ExecutionBinding::Bind(address) => format!(
                    "bind {:?}#{}",
                    compiled.node_ids[address.node_idx], address.port_idx
                ),
            };
            out.push(format!(
                "  in required={} fs_path={} {binding}",
                input.required, input.stamps_fs_path
            ));
        }
        for output in &compiled.outputs[e_node.outputs] {
            out.push(format!("  out {output:?}"));
        }
        for event in &compiled.events[e_node.events] {
            let subscribers: Vec<_> = event
                .subscribers
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

/// A sink reading a source, plus an unwired node — enough for every question
/// the artifact answers about an authored node to have a distinct answer.
fn fixture() -> TestGraph {
    let mut g = TestGraph::new();
    g.add("source", |n| n.pure().output(DataType::Int));
    g.add("sink", |n| n.records());
    g.add("loose", |n| n.pure().output(DataType::Int));
    g.wire("source", 0, "sink", 0);
    g
}

/// Every authored node holds compiled work, whatever its role — a source, its
/// reader, and a node nothing wires to alike. An id the program never held does
/// not.
#[test]
fn the_artifact_answers_for_each_authored_node() {
    let g = fixture();
    let compiled = g.compile();

    for name in ["source", "sink", "loose"] {
        assert!(compiled.program.contains(g.id(name)));
    }
    assert!(!compiled.program.contains(NodeId::unique()));
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
            .event("tick", EventLambda::default())
    });
    warmup.add("consumer", |n| {
        n.sink().input(DataType::String).input(DataType::Float)
    });
    warmup.wire("emitter", 0, "consumer", 0);
    warmup.wire("emitter", 1, "consumer", 1);
    warmup.subscribe("emitter", 0, "consumer");

    let subject = fixture();
    let authored: Vec<NodeId> = ["source", "sink", "loose"]
        .iter()
        .map(|name| subject.id(name))
        .collect();

    let mut reused = Compiler::default();
    reused.compile(&warmup.graph, &warmup.library).unwrap();
    let after_reuse = reused.compile(&subject.graph, &subject.library).unwrap();
    let fresh = Compiler::default()
        .compile(&subject.graph, &subject.library)
        .unwrap();

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
    let g = fixture();
    let authored: Vec<NodeId> = ["source", "sink", "loose"]
        .iter()
        .map(|name| g.id(name))
        .collect();
    let mut compiler = Compiler::default();

    let first = compiler.compile(&g.graph, &g.library).unwrap();
    let first_summary = summary(&first, &authored);
    let second = compiler.compile(&g.graph, &g.library).unwrap();

    assert_eq!(summary(&second, &authored), first_summary);
    assert_eq!(second.inputs.len(), first.inputs.len());
    assert_eq!(second.outputs.len(), first.outputs.len());
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
    g.wire("producer", 0, "consumer", 0);
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
    for (position, node_id) in expected.iter().enumerate() {
        assert_eq!(
            compiled.program.node(*node_id).unwrap(),
            NodeIdx(position as u32)
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
        compiled.program.outputs.len(),
        1,
        "one output port across both nodes, typed from the producer's declaration"
    );
    assert_eq!(compiled.output_types("producer"), [DataType::Int]);
    assert_eq!(compiled.program.inputs.len(), 1);
    assert_eq!(producer.outputs.start, 0, "the only run starts the pool");
}

/// A wire is interned to the producer's dense address as the walk resolves it —
/// the right node index and the port index the authored wire named.
#[test]
fn interns_a_wire_to_the_producers_dense_address() {
    let g = wired_pair(DataType::Int, DataType::Int);
    let compiled = g.compile();

    let ExecutionBinding::Bind(address) = *compiled.binding("consumer", 0) else {
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

/// The type gate: a wire whose producer no longer fits the consumer lowers as
/// unbound rather than severing the authored wiring, so it revives when the
/// types line up again.
#[test]
fn drops_a_type_mismatched_wire_to_unbound() {
    let g = wired_pair(DataType::String, DataType::Int);
    let compiled = g.compile();

    assert!(
        matches!(compiled.binding("consumer", 0), ExecutionBinding::None),
        "a String producer does not satisfy an Int input"
    );
    assert!(
        g.graph
            .bindings
            .contains_key(&InputPort::new(g.id("consumer"), 0)),
        "the authored wire itself is untouched"
    );
}

/// A const that satisfies its input travels through verbatim; a mismatched one
/// lowers unbound, on the same terms as a wire.
#[test]
fn keeps_a_satisfying_const_and_drops_a_mismatched_one() {
    for (value, kept) in [
        (StaticValue::Int(7), true),
        (StaticValue::String("no".to_owned()), false),
    ] {
        let mut g = pair(DataType::Int, DataType::Int);
        g.constant("consumer", 0, value.clone());
        assert_eq!(
            matches!(
                g.compile().binding("consumer", 0),
                ExecutionBinding::Const(_)
            ),
            kept,
            "const {value:?} on an Int input",
        );
    }
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

/// The walk keeps only scratch, so one `Compiler` serves every compile and a
/// second walk cannot observe the first.
#[test]
fn a_second_walk_cannot_observe_the_first() {
    let mut g = TestGraph::new();
    g.add("producer", |n| n.pure().output(DataType::Int));
    let mut compiler = Compiler::default();
    compiler.compile(&g.graph, &g.library).unwrap();
    let program = compiler.compile(&g.graph, &g.library).unwrap();

    assert_eq!(program.e_nodes.len(), 1);
    assert_eq!(
        program.outputs.len(),
        1,
        "the port pools restart from empty"
    );
    assert!(program.events.is_empty());
}

/// The walk stamps each output with the type it *resolved*, wildcards followed —
/// so the program carries a type no declaration in the library mentions, rather
/// than the `Any` a re-derivation off the declaration alone would produce.
#[test]
fn stamps_each_output_with_the_resolved_type() {
    let mut g = TestGraph::new();
    g.add("producer", |n| n.pure().output(DataType::String));
    g.add("pass", |n| n.pure().input(DataType::Any).wildcard(0));
    g.wire("producer", 0, "pass", 0);

    assert_eq!(
        g.compile().output_types("pass"),
        [DataType::String],
        "the wildcard followed its mirror to the producer's String"
    );
}

/// Each event port gets its declared lambda and exactly the subscribers this
/// graph wires to it, so an empty list means "nothing subscribes" rather than
/// "not wired yet" — including for an emitter placed after its subscriber.
#[test]
fn wires_each_event_with_the_subscribers_resolved_for_it() {
    let mut g = TestGraph::new();
    g.add("emitter", |n| {
        n.event("quiet", EventLambda::default())
            .event("subscribed", EventLambda::default())
    });
    g.add("listener", |n| n.sink());
    g.subscribe("emitter", 1, "listener");

    let compiled = g.compile();
    assert_eq!(compiled.node("emitter").events.len, 2);
    assert!(
        compiled.subscribers("emitter", 0).is_empty(),
        "the unsubscribed port carries no subscriber"
    );
    assert_eq!(compiled.subscribers("emitter", 1), ["listener"]);
}

/// A subscription whose emitter or subscriber is disabled wires nothing, and
/// neither does one naming an event the func no longer declares — the same
/// drift tolerance the type gate applies to data edges.
#[test]
fn drops_subscriptions_that_cannot_fire() {
    /// Which end of the edge, if either, the document disabled.
    #[derive(Clone, Copy, Debug)]
    enum Disabled {
        Neither,
        Emitter,
        Subscriber,
    }
    let build = |events: usize, disabled: Disabled| {
        let mut g = TestGraph::new();
        g.add("emitter", move |mut n| {
            for i in 0..events {
                n = n.event(&format!("e{i}"), EventLambda::default());
            }
            n
        });
        g.add("listener", |n| n.sink());
        // Authored against a two-event declaration; `events == 1` is the library
        // having since dropped the port this names.
        g.subscribe("emitter", 1, "listener");
        match disabled {
            Disabled::Neither => {}
            Disabled::Emitter => g.disable("emitter"),
            Disabled::Subscriber => g.disable("listener"),
        }
        let compiled = g.compile();
        (0..events)
            .map(|event_idx| compiled.subscribers("emitter", event_idx).len())
            .sum::<usize>()
    };

    assert_eq!(
        build(2, Disabled::Neither),
        1,
        "the live subscription wires"
    );
    assert_eq!(
        build(2, Disabled::Emitter),
        0,
        "a disabled emitter fires nothing"
    );
    assert_eq!(
        build(2, Disabled::Subscriber),
        0,
        "a disabled subscriber receives nothing"
    );
    assert_eq!(
        build(1, Disabled::Neither),
        0,
        "an event the func dropped wires nothing"
    );
}

/// A wire naming a port the producer no longer declares lowers as unbound, and
/// the resolved-type table is *not* the thing that decides it.
///
/// A wildcard chain records every port it walks through — out-of-range ones
/// included — as `Any`, so the consumer here reads `Some(Any)` for a port that
/// does not exist. Only the range check against the producer's declared count
/// unbinds it. Take that check out and the artifact carries an `OutputAddr`
/// pointing past its producer's output run.
#[test]
fn a_wildcard_chain_does_not_make_a_dropped_port_bindable() {
    let mut g = TestGraph::new();
    g.add("producer", |n| n.pure().output(DataType::Int));
    g.add("pass", |n| n.pure().input(DataType::Any).wildcard(0));
    // Port 99 does not exist: the library shrank under a saved document.
    let dropped = OutputPort::new(g.id("producer"), 99);
    g.wire("producer", 99, "pass", 0);

    let mut types = OutputTypes::default();
    types.update(&g.graph, &g.library);
    assert_eq!(
        types.get(dropped),
        Some(&DataType::Any),
        "the chain stamps the port it walked through, declared or not"
    );

    assert!(
        matches!(g.compile().binding("pass", 0), ExecutionBinding::None),
        "the range check unbinds a port the producer does not declare"
    );
    assert!(
        g.graph
            .bindings
            .contains_key(&InputPort::new(g.id("pass"), 0)),
        "the authored wire itself is untouched"
    );
}
