//! The compile's flatten: a document with subgraphs lowers to the program its hand-inlined twin
//! does, each node inside an instance under the path of instances down to it.

use std::collections::{BTreeMap, HashMap};

use super::*;
use crate::graph::interface::Interface;
use crate::graph::ports::{InputDecl, Numbered, OutputDecl, Ports};
use crate::graph::subgraph::Subgraph;
use crate::internals::engine::TestEngine;
use crate::internals::test_graph::sets::{
    ADD, CONFIG_SINK, SINK, SRC, TICKER, library, n, node, s,
};

fn path(ids: &[u128]) -> ProgramId {
    ProgramId::new(&ids.iter().copied().map(n).collect::<Vec<_>>())
}

const ADDER: SubgraphId = s(50);
const TWICE: SubgraphId = s(60);

fn wire(graph: &mut Graph, consumer: (NodeId, PortId), producer: (NodeId, PortId)) {
    graph.set_input_binding(
        InputPort::new(consumer.0, consumer.1),
        Binding::wire(producer.0, producer.1),
    );
}

/// The first port of each func the library declares, as it numbers them; `add`'s `b` is 2.
const FIRST: PortId = PortId::new(1);
const SECOND: PortId = PortId::new(2);
/// The adder's interface ports, on its instances and its boundary nodes alike.
const A: PortId = PortId::new(101);
const B: PortId = PortId::new(102);
const SUM: PortId = PortId::new(201);
const PASS: PortId = PortId::new(202);

/// Inputs `a` and optional `b`, defaulting to 5; outputs `sum` and `pass`.
fn adder_interface() -> Interface {
    Interface::new(Ports {
        inputs: vec![
            Numbered::new(A, InputDecl::required("a", DataType::Int)),
            Numbered::new(B, InputDecl::optional("b", DataType::Int).default(5i64)),
        ],
        outputs: vec![
            Numbered::new(SUM, OutputDecl::new("sum", DataType::Int)),
            Numbered::new(PASS, OutputDecl::new("pass", DataType::Int)),
        ],
    })
}

/// `adder`: inputs `a` and `b` into node 13, an `add`; outputs its `sum`, and `pass`, which is
/// `a` wired straight through.
fn adder(library: &Library) -> Subgraph {
    let mut subgraph = Subgraph::new("adder", adder_interface());
    let inputs = subgraph.inputs_node();
    let outputs = subgraph.outputs_node();
    let graph = &mut subgraph.graph;
    graph.insert(n(13), node(library, ADD));
    wire(graph, (n(13), FIRST), (inputs, A));
    wire(graph, (n(13), SECOND), (inputs, B));
    wire(graph, (outputs, SUM), (n(13), FIRST));
    wire(graph, (outputs, PASS), (inputs, A));
    subgraph
}

/// A document's graphs and the library they resolve against.
#[derive(Debug, Default)]
struct Doc {
    graphs: GraphSet,
    library: Library,
}

impl Doc {
    fn compile(&self) -> CompiledGraph {
        Compiler::default().compile(&self.graphs, &self.library)
    }

    fn main(&mut self) -> &mut Graph {
        self.graphs.main_mut()
    }

    /// Place an instance of `subgraph` in the main graph under `node_id`.
    fn place(&mut self, node_id: NodeId, subgraph: SubgraphId) {
        let instance = Node::instance(subgraph);
        self.main().insert(node_id, instance);
    }
}

/// Main: `src` 1 into instance 2 of `adder`, whose `b` holds 7; `sink` 3 reads the sum and `sink`
/// 4 the passthrough.
fn adder_doc() -> Doc {
    let mut doc = Doc {
        library: library(),
        ..Doc::default()
    };
    doc.graphs.insert_subgraph(ADDER, adder(&doc.library));
    let [src, sink] = [SRC, SINK].map(|func_id| node(&doc.library, func_id));
    doc.main().insert(n(1), src);
    doc.place(n(2), ADDER);
    doc.main().insert(n(3), sink.clone());
    doc.main().insert(n(4), sink);
    let main = doc.main();
    wire(main, (n(2), A), (n(1), FIRST));
    main.set_input_binding(InputPort::new(n(2), B), Binding::Const(7i64.into()));
    wire(main, (n(3), FIRST), (n(2), SUM));
    wire(main, (n(4), FIRST), (n(2), PASS));
    doc
}

/// Every program node's lowered form under its label, its bindings and subscribers by the label
/// they name: two programs compare node for node whatever ids they place them under.
fn labelled(
    compiled: &CompiledGraph,
    labels: &HashMap<ProgramId, &str>,
) -> BTreeMap<String, Vec<String>> {
    let label = |node_idx: NodeIdx| labels[&compiled.program_ids[node_idx]].to_owned();
    compiled
        .e_nodes
        .iter_indexed()
        .map(|(node_idx, e_node)| {
            let mut lines = vec![format!(
                "func={:?} sink={} disabled={} cache={:?} outputs={}",
                e_node.func_ref.id(),
                e_node.sink,
                e_node.disabled,
                e_node.cache,
                e_node.outputs.len,
            )];
            lines.extend(
                compiled.inputs[e_node.inputs]
                    .iter()
                    .map(|input| match &input.binding {
                        ExecBinding::None => "none".to_owned(),
                        ExecBinding::Const(value) => format!("const {value:?}"),
                        ExecBinding::Wire(address) => {
                            format!("wire {}#{}", label(address.node_idx), address.port_idx)
                        }
                    }),
            );
            lines.extend(compiled.events[e_node.events].iter().map(|event| {
                let subscribers: Vec<String> = compiled.subscribers[event.subscribers]
                    .iter()
                    .map(|&subscriber| label(subscriber))
                    .collect();
                format!("subscribers {subscribers:?}")
            }));
            (label(node_idx), lines)
        })
        .collect()
}

/// The proposal's own test: a graph with a subgraph compiles to the program of its twin with the
/// subgraph's nodes written in by hand. The instance and both boundary nodes emit nothing; the
/// `add` inside reads `src` across the `Inputs` node and the instance's constant across both; the
/// passthrough sink reads `src` directly, as nothing stands between them once the boundary goes.
#[test]
fn a_subgraph_compiles_to_its_hand_inlined_twin() {
    let doc = adder_doc();
    let compiled = doc.compile();
    assert_eq!(
        compiled.program_ids.iter().copied().collect::<Vec<_>>(),
        [path(&[1]), path(&[2, 13]), path(&[3]), path(&[4])],
        "the add sorts under its instance, between nodes 1 and 3"
    );

    let mut twin = Doc {
        library: library(),
        ..Doc::default()
    };
    for (node_id, func_id) in [(1, SRC), (13, ADD), (3, SINK), (4, SINK)] {
        let node = node(&twin.library, func_id);
        twin.main().insert(n(node_id), node);
    }
    let main = twin.main();
    wire(main, (n(13), FIRST), (n(1), FIRST));
    main.set_input_binding(InputPort::new(n(13), SECOND), Binding::Const(7i64.into()));
    wire(main, (n(3), FIRST), (n(13), FIRST));
    wire(main, (n(4), FIRST), (n(1), FIRST));

    let labels = HashMap::from([
        (path(&[1]), "src"),
        (path(&[2, 13]), "add"),
        (path(&[13]), "add"),
        (path(&[3]), "sum sink"),
        (path(&[4]), "pass sink"),
    ]);
    let flattened = labelled(&compiled, &labels);
    assert_eq!(flattened, labelled(&twin.compile(), &labels));
    assert_eq!(
        flattened["add"][1..],
        ["wire src#0".to_owned(), "const Int(7)".to_owned()]
    );
    assert_eq!(flattened["pass sink"][1..], ["wire src#0".to_owned()]);
}

/// An instance's cache mode joins the node inside it that makes a value leaving it, at that
/// instance's path alone: with `adder` placed twice and instance 2 kept in RAM, its `add` keeps
/// its value and instance 5's does not; `pass`, which only passes `src` through, leaves `src` as
/// it was; and an `add` that saves to disk keeps both bits.
#[test]
fn an_instances_cache_mode_caches_the_values_that_leave_it() {
    let cache_of = |doc: &Doc, path: ProgramId| {
        let compiled = doc.compile();
        compiled[compiled.node(path).unwrap()].cache
    };
    let mut doc = adder_doc();
    doc.place(n(5), ADDER);
    wire(doc.main(), (n(5), A), (n(1), FIRST));
    *doc.main().find_mut(n(2)).unwrap().cache_mut().unwrap() = CacheMode::Ram;
    assert_eq!(cache_of(&doc, path(&[2, 13])), CacheMode::Ram);
    assert_eq!(cache_of(&doc, path(&[5, 13])), CacheMode::None);
    assert_eq!(
        cache_of(&doc, path(&[1])),
        CacheMode::None,
        "a passthrough's producer"
    );

    *doc.graphs
        .graph_mut(GraphId::Subgraph(ADDER))
        .find_mut(n(13))
        .unwrap()
        .cache_mut()
        .unwrap() = CacheMode::Disk;
    assert_eq!(cache_of(&doc, path(&[2, 13])), CacheMode::Both);
    assert_eq!(cache_of(&doc, path(&[5, 13])), CacheMode::Disk);
}

/// A wire from a second `Inputs` node reads what the instance holds as one from the first does:
/// with `add`'s `b` moved onto `Inputs` node 14, the program is the one the first node gives.
#[test]
fn a_second_inputs_node_reads_the_instances_bindings_as_the_first_does() {
    let doc = adder_doc();
    let mut two = adder_doc();
    let adder = two.graphs.graph_mut(GraphId::Subgraph(ADDER));
    adder.insert(n(14), Node::from(Boundary::Inputs));
    wire(adder, (n(13), SECOND), (n(14), B));
    let labels = HashMap::from([
        (path(&[1]), "src"),
        (path(&[2, 13]), "add"),
        (path(&[3]), "sum sink"),
        (path(&[4]), "pass sink"),
    ]);
    let compiled = labelled(&two.compile(), &labels);
    assert_eq!(compiled, labelled(&doc.compile(), &labels));
    assert_eq!(
        compiled["add"][1..],
        ["wire src#0".to_owned(), "const Int(7)".to_owned()]
    );
}

/// A const-only interface input reaches a const-only input inside by a wire from the `Inputs`
/// node, and the instance holds a constant there alone, so the wire lowers to that constant.
#[test]
fn a_const_only_port_lowers_to_the_instances_constant() {
    const CONFIGURED: SubgraphId = s(80);
    const CONFIG: PortId = PortId::new(301);
    let library = library();
    let mut configured = Subgraph::new(
        "configured",
        Interface::new(Ports {
            inputs: vec![Numbered::new(
                CONFIG,
                InputDecl::required("config", DataType::Int).const_only(),
            )],
            outputs: vec![],
        }),
    );
    let inputs = configured.inputs_node();
    configured.graph.insert(n(31), node(&library, CONFIG_SINK));
    wire(&mut configured.graph, (n(31), FIRST), (inputs, CONFIG));
    let mut doc = Doc {
        library,
        ..Doc::default()
    };
    doc.graphs.insert_subgraph(CONFIGURED, configured);
    doc.place(n(1), CONFIGURED);
    doc.main()
        .set_input_binding(InputPort::new(n(1), CONFIG), Binding::Const(3i64.into()));

    let compiled = doc.compile();
    let program = labelled(&compiled, &HashMap::from([(path(&[1, 31]), "config sink")]));
    assert_eq!(program["config sink"][1..], ["const Int(3)".to_owned()]);
}

/// Two levels down, a node's id is the path of both instances, and a binding is followed out
/// through both boundaries: `add`'s `a` reaches `src` in the main graph, and its `b`, which no
/// level binds, lowers to nothing.
#[test]
fn a_nested_node_is_named_by_its_path_and_reads_across_both_boundaries() {
    let mut doc = adder_doc();
    let adder = doc.graphs.subgraph(ADDER).unwrap();
    let mut twice = Subgraph::new("twice", adder.interface.clone());
    let inputs = twice.inputs_node();
    let outputs = twice.outputs_node();
    twice.graph.insert(n(21), Node::instance(ADDER));
    wire(&mut twice.graph, (n(21), A), (inputs, A));
    wire(&mut twice.graph, (n(21), B), (inputs, B));
    wire(&mut twice.graph, (outputs, SUM), (n(21), SUM));
    doc.graphs.insert_subgraph(TWICE, twice);

    doc.place(n(31), TWICE);
    wire(doc.main(), (n(31), A), (n(1), FIRST));
    wire(doc.main(), (n(3), FIRST), (n(31), SUM));

    let compiled = doc.compile();
    assert_eq!(
        compiled.program_ids.iter().copied().collect::<Vec<_>>(),
        [
            path(&[1]),
            path(&[2, 13]),
            path(&[3]),
            path(&[4]),
            path(&[31, 21, 13]),
        ]
    );
    let labels = HashMap::from([
        (path(&[1]), "src"),
        (path(&[2, 13]), "add in 2"),
        (path(&[31, 21, 13]), "add in 31"),
        (path(&[3]), "sum sink"),
        (path(&[4]), "pass sink"),
    ]);
    let program = labelled(&compiled, &labels);
    assert_eq!(
        program["add in 31"][1..],
        ["wire src#0".to_owned(), "none".to_owned()]
    );
    assert_eq!(program["sum sink"][1..], ["wire add in 31#0".to_owned()]);
}

/// Each instance expands its subgraph apart: two instances of `adder` are two `add` nodes, each
/// reading its own instance's inputs. A disabled instance disables every node inside it and
/// nothing outside.
#[test]
fn each_instance_expands_apart_and_a_disabled_one_disables_its_nodes() {
    let mut doc = adder_doc();
    doc.place(n(5), ADDER);
    let main = doc.main();
    main.set_input_binding(InputPort::new(n(5), A), Binding::Const(2i64.into()));
    main.set_input_binding(InputPort::new(n(5), B), Binding::Const(3i64.into()));
    main.find_mut(n(5)).unwrap().disabled = true;

    let compiled = doc.compile();
    let labels = HashMap::from([
        (path(&[1]), "src"),
        (path(&[2, 13]), "add in 2"),
        (path(&[5, 13]), "add in 5"),
        (path(&[3]), "sum sink"),
        (path(&[4]), "pass sink"),
    ]);
    let program = labelled(&compiled, &labels);
    assert_eq!(
        program["add in 2"],
        [
            format!("func={ADD:?} sink=false disabled=false cache=None outputs=1"),
            "wire src#0".to_owned(),
            "const Int(7)".to_owned(),
        ]
    );
    assert_eq!(
        program["add in 5"],
        [
            format!("func={ADD:?} sink=false disabled=true cache=None outputs=1"),
            "const Int(2)".to_owned(),
            "const Int(3)".to_owned(),
        ]
    );
    assert!(program["src"][0].contains("disabled=false"));
}

/// Whether a program writes to disk is a question about its program nodes: an `add` inside the
/// instance set to `Disk` makes it so though no node of the main graph is.
#[test]
fn a_node_inside_an_instance_makes_the_program_persist_to_disk() {
    let mut doc = adder_doc();
    assert!(!doc.compile().persists_to_disk());
    let inner = doc
        .graphs
        .graph_mut(GraphId::Subgraph(ADDER))
        .find_mut(n(13))
        .unwrap();
    *inner.cache_mut().unwrap() = CacheMode::Disk;
    assert!(doc.compile().persists_to_disk());
}

/// A subscription inside a subgraph wires the emitter and subscriber of the same instance, and
/// none of another; inside a disabled instance it wires nothing, as between disabled nodes
/// anywhere.
#[test]
fn a_subscription_inside_a_subgraph_stays_inside_each_instance() {
    let library = library();
    let mut ticking = Subgraph::new("ticking", Interface::default());
    ticking.graph.insert(n(41), node(&library, TICKER));
    ticking.graph.insert(n(42), node(&library, SINK));
    ticking.graph.subscribe(EventPort::new(n(41), FIRST), n(42));
    let id = s(70);
    let mut doc = Doc {
        library,
        ..Doc::default()
    };
    doc.graphs.insert_subgraph(id, ticking);
    for instance in [n(1), n(2)] {
        doc.place(instance, id);
    }
    doc.main().find_mut(n(2)).unwrap().disabled = true;

    let compiled = doc.compile();
    let labels = HashMap::from([
        (path(&[1, 41]), "ticker in 1"),
        (path(&[1, 42]), "sink in 1"),
        (path(&[2, 41]), "ticker in 2"),
        (path(&[2, 42]), "sink in 2"),
    ]);
    let program = labelled(&compiled, &labels);
    assert_eq!(program["ticker in 1"][1..], ["subscribers [\"sink in 1\"]"]);
    assert_eq!(program["ticker in 2"][1..], ["subscribers []"]);
}

/// An event crosses a subgraph's boundary on its interface. `relay` fires its event `out` 301
/// when the trigger `in` 401 is heard, by its `Inputs` node firing it into its `Outputs` node; `outer` places
/// a `relay` 61 inside, wiring `outer`'s trigger into it and its event out, so an event goes two
/// levels in and two out. In the main graph `ticker` 1 fires into instance 2's trigger, instance
/// 2's event lands on `sink` 3, and a disabled instance 4 passes nothing: the ticker's
/// subscribers are the sink alone, with no node in between, and a ticker inside `relay` that
/// fires its `Outputs` node reaches the main graph's sink too.
#[test]
fn an_event_crosses_boundaries_on_the_interface() {
    use crate::graph::identity::TriggerPort;
    use crate::graph::interface::EventPorts;
    use crate::graph::ports::EventDecl;
    use crate::graph::subscription::Listener;

    const OUT: PortId = PortId::new(301);
    const IN: PortId = PortId::new(401);
    let library = library();
    let events = || {
        Interface::default()
            .with_event_ports(EventPorts {
                events: vec![Numbered::new(OUT, EventDecl::new("out"))],
                triggers: vec![Numbered::new(IN, EventDecl::new("in"))],
            })
            .unwrap()
    };
    let (relay_id, outer_id) = (s(80), s(81));
    let mut relay = Subgraph::new("relay", events());
    let (relay_in, relay_out) = (relay.inputs_node(), relay.outputs_node());
    relay.graph.insert(n(53), node(&library, TICKER));
    relay.graph.subscribe(
        EventPort::new(relay_in, IN),
        Listener::Trigger(TriggerPort::new(relay_out, OUT)),
    );
    relay.graph.subscribe(
        EventPort::new(n(53), FIRST),
        Listener::Trigger(TriggerPort::new(relay_out, OUT)),
    );
    let mut outer = Subgraph::new("outer", events());
    let (outer_in, outer_out) = (outer.inputs_node(), outer.outputs_node());
    outer.graph.insert(n(61), Node::instance(relay_id));
    outer.graph.subscribe(
        EventPort::new(outer_in, IN),
        Listener::Trigger(TriggerPort::new(n(61), IN)),
    );
    outer.graph.subscribe(
        EventPort::new(n(61), OUT),
        Listener::Trigger(TriggerPort::new(outer_out, OUT)),
    );
    let mut doc = Doc {
        library,
        ..Doc::default()
    };
    doc.graphs.insert_subgraph(relay_id, relay);
    doc.graphs.insert_subgraph(outer_id, outer);
    let (ticker, sink) = (node(&doc.library, TICKER), node(&doc.library, SINK));
    doc.main().insert(n(1), ticker);
    doc.place(n(2), outer_id);
    doc.main().insert(n(3), sink);
    doc.place(n(4), outer_id);
    doc.main().find_mut(n(4)).unwrap().disabled = true;
    let main = doc.main();
    main.subscribe(
        EventPort::new(n(1), FIRST),
        Listener::Trigger(TriggerPort::new(n(2), IN)),
    );
    main.subscribe(
        EventPort::new(n(1), FIRST),
        Listener::Trigger(TriggerPort::new(n(4), IN)),
    );
    main.subscribe(EventPort::new(n(2), OUT), n(3));
    main.subscribe(EventPort::new(n(4), OUT), n(3));
    doc.graphs.validate_with(&doc.library).unwrap();

    let compiled = doc.compile();
    let labels = HashMap::from([
        (path(&[1]), "ticker"),
        (path(&[3]), "sink"),
        (path(&[2, 61, 53]), "inner ticker"),
        (path(&[4, 61, 53]), "disabled inner ticker"),
    ]);
    let program = labelled(&compiled, &labels);
    assert_eq!(program["ticker"][1..], ["subscribers [\"sink\"]"]);
    assert_eq!(program["inner ticker"][1..], ["subscribers [\"sink\"]"]);
    assert_eq!(program["disabled inner ticker"][1..], ["subscribers []"]);
}

/// A program the flatten built runs: the sum inside the instance reaches the sink in the main
/// graph, `4 + 7`, and the passthrough hands `src`'s 4 on unchanged.
#[tokio::test]
async fn a_flattened_program_runs() {
    let mut g = TestGraph::new();
    g.add("src", |n| {
        n.pure().output(DataType::Int).compute(|_| 4i64.into())
    });
    g.add("sum", NodeSpec::sum);
    g.add("sum sink", NodeSpec::records);
    g.add("pass sink", NodeSpec::records);
    let sum = g
        .remove("sum")
        .node()
        .decl(g.scope())
        .func()
        .expect("`sum` has a body")
        .clone();

    let mut subgraph = Subgraph::new("adder", adder_interface());
    let inputs = subgraph.inputs_node();
    let outputs = subgraph.outputs_node();
    let mut inner = Node::from(&sum);
    *inner.cache_mut().unwrap() = CacheMode::None;
    subgraph.graph.insert(n(13), inner);
    wire(&mut subgraph.graph, (n(13), FIRST), (inputs, A));
    wire(&mut subgraph.graph, (n(13), SECOND), (inputs, B));
    wire(&mut subgraph.graph, (outputs, SUM), (n(13), FIRST));
    wire(&mut subgraph.graph, (outputs, PASS), (inputs, A));
    let placed = subgraph.clone();
    g.graphs.insert_subgraph(ADDER, subgraph);

    let [src, sum_sink, pass_sink] = ["src", "sum sink", "pass sink"].map(|name| g.id(name));
    let main = g.graph_mut();
    let instance = main.add_instance(ADDER, &placed);
    main.set_input_binding(InputPort::new(instance, A), Binding::wire(src, FIRST));
    main.set_input_binding(InputPort::new(instance, B), Binding::Const(7i64.into()));
    main.set_input_binding(
        InputPort::new(sum_sink, FIRST),
        Binding::wire(instance, SUM),
    );
    main.set_input_binding(
        InputPort::new(pass_sink, FIRST),
        Binding::wire(instance, PASS),
    );

    let mut e = TestEngine::over(g);
    let run = e.run_sinks().await;
    let mut logs = run.logs();
    logs.sort_unstable();
    assert_eq!(logs, ["11", "4"]);
}
