use std::collections::BTreeMap;
use std::panic;
use std::panic::AssertUnwindSafe;

use common::{ErrorChain, SerdeFormat};

use crate::data::data_type::DataType;
use crate::data::data_type::custom_type::CustomTypeId;
use crate::graph::Graph;
use crate::graph::binding::Binding;
use crate::graph::binding_entry::BindingEntry;
use crate::graph::detached::{DetachedNode, DetachedNodeParts};
use crate::graph::error::{
    BindingRejection, DetachReason, GraphValidationError, PlacementRejection,
    SubscriptionRejection, Unresolved,
};
use crate::graph::graph_set::{GraphSet, InterfacePort, ProgramShape, UnknownType};
use crate::graph::identity::{
    EventPort, FuncId, GraphId, InputPort, NodeAddr, NodeId, OutputPort, PortId, SubgraphId,
};
use crate::graph::instance_path::InstancePath;
use crate::graph::interface::Interface;
use crate::graph::interface_change::{ChangeSide, DroppedBinding};
use crate::graph::node::{Boundary, Node};
use crate::graph::node_decl::NodeDecl;
use crate::graph::ports::{InputDecl, OutputDecl, PortList, Ports, numbered_port};
use crate::graph::reconciled::Repair;
use crate::graph::stub::Stub;
use crate::graph::subgraph::Subgraph;
use crate::graph::subscription::Subscription;
use crate::internals;
use crate::internals::test_graph::sets::{
    ADD, ANY_SINK, CONFIG_SINK, SINK, SRC, TICKER, library, n, node, s,
};
use crate::library::Library;

/// A subgraph with no ports.
fn empty(name: &str) -> Subgraph {
    Subgraph::new(name, Interface::default())
}

/// Put an instance of `placed` into `graph` under `node_id`.
fn place(set: &mut GraphSet, graph: GraphId, node_id: NodeId, placed: SubgraphId) {
    let node = Node::instance(placed, set.subgraph(placed).unwrap());
    set.graph_mut(graph).insert(node_id, node);
}

/// The repairs that took a node out into a stub, as `(node, reason)`.
fn stubbed(repairs: &[Repair]) -> Vec<(NodeId, DetachReason)> {
    repairs
        .iter()
        .filter_map(|repair| match repair {
            Repair::Stubbed { node_id, reason } => Some((*node_id, *reason)),
            _ => None,
        })
        .collect()
}

/// Subgraph 1 places 2, and 2 places nothing: a loop closes only by placing 1 inside 2 or
/// itself, and the main graph never closes one.
#[test]
fn a_placement_that_closes_a_loop_is_refused() {
    let mut set = GraphSet::default();
    set.insert_subgraph(s(1), empty("outer"));
    set.insert_subgraph(s(2), empty("inner"));
    place(&mut set, GraphId::Subgraph(s(1)), n(10), s(2));
    let instance = |placed| Node::instance(placed, set.subgraph(placed).unwrap());
    let admit = |graph, placed| set.admit_nodes(graph, [&instance(placed)]);

    assert_eq!(
        admit(GraphId::Subgraph(s(2)), s(1)),
        Err(PlacementRejection::Recursive)
    );
    assert_eq!(
        admit(GraphId::Subgraph(s(1)), s(1)),
        Err(PlacementRejection::Recursive)
    );
    assert_eq!(
        admit(GraphId::Subgraph(s(2)), s(2)),
        Err(PlacementRejection::Recursive)
    );
    assert_eq!(admit(GraphId::Subgraph(s(1)), s(2)), Ok(()));
    assert_eq!(admit(GraphId::Main, s(1)), Ok(()));

    // Subgraph 1 places nothing yet, so a walk in id order reaches it before 2: an instance of 2
    // added to it is measured all the same.
    let mut apart = GraphSet::default();
    apart.insert_subgraph(s(1), empty("first"));
    apart.insert_subgraph(s(2), empty("second"));
    let second = Node::instance(s(2), apart.subgraph(s(2)).unwrap());
    assert_eq!(
        apart.admit_nodes(GraphId::Subgraph(s(1)), [&second]),
        Ok(())
    );
}

/// A set where 1 places 2 and 2 places 1 is refused, naming the instance a walk from the lowest
/// id meets on the path: 2's, as the walk enters 1, then 2, and returns to 1. A load keeps
/// exactly that one as a stub, after which the set validates; a second load attaches it, finds
/// the same loop, and keeps it as it was, so it reports nothing.
#[test]
fn a_loop_is_refused_and_a_load_breaks_it_at_one_instance() {
    let library = library();
    let mut set = GraphSet::default();
    set.insert_subgraph(s(1), empty("one"));
    set.insert_subgraph(s(2), empty("two"));
    place(&mut set, GraphId::Subgraph(s(1)), n(12), s(2));
    place(&mut set, GraphId::Subgraph(s(2)), n(21), s(1));

    let error = set.validate_with(&library).unwrap_err();
    assert_eq!(error.graph, GraphId::Subgraph(s(2)));
    assert!(matches!(
        error.error,
        GraphValidationError::Recursive { node_id } if node_id == n(21)
    ));

    let report = set.reconcile(&library);
    assert!(report.main.is_empty());
    assert_eq!(report.subgraphs.keys().copied().collect::<Vec<_>>(), [s(2)]);
    assert_eq!(
        report.subgraphs[&s(2)].repairs,
        [Repair::Stubbed {
            node_id: n(21),
            reason: DetachReason::Recursive,
        }]
    );
    let two = set.graph(GraphId::Subgraph(s(2)));
    assert_eq!(two.stub(n(21)).unwrap().reason, DetachReason::Recursive);
    assert!(two.find(n(21)).is_none());
    assert!(set.graph(GraphId::Subgraph(s(1))).find(n(12)).is_some());
    set.validate_with(&library).unwrap();

    let reloaded = set.clone();
    assert!(set.reconcile(&library).is_empty());
    assert_eq!(set, reloaded);
}

/// A chain of eight subgraphs, each placing the next and the last holding a `src`, nests eight
/// levels: the last is 1, each one up adds one. The main graph placing the first would nest
/// nine, past [`ProgramShape::MAX_LEVELS`], so the gate refuses the add and a load keeps that
/// one instance as a stub, keeping the chain itself.
#[test]
fn a_ninth_level_is_refused_and_detached() {
    let library = library();
    let mut set = GraphSet::default();
    for id in 1..=8 {
        set.insert_subgraph(s(id), empty("link"));
    }
    for id in 1..8 {
        place(&mut set, GraphId::Subgraph(s(id)), n(100 + id), s(id + 1));
    }
    set.graph_mut(GraphId::Subgraph(s(8)))
        .insert(n(1), node(&library, SRC));
    set.main_mut().insert(n(1), node(&library, SRC));
    let first = Node::instance(s(1), set.subgraph(s(1)).unwrap());
    assert_eq!(
        set.admit_nodes(GraphId::Main, [&first]),
        Err(PlacementRejection::TooDeep {
            graph: GraphId::Main,
            levels: 9
        })
    );
    let second = Node::instance(s(2), set.subgraph(s(2)).unwrap());
    assert_eq!(set.admit_nodes(GraphId::Main, [&second]), Ok(()));

    set.main_mut().insert(n(2), first);
    let error = set.validate_with(&library).unwrap_err();
    assert_eq!(error.graph, GraphId::Main);
    assert!(matches!(
        error.error,
        GraphValidationError::TooDeep { levels: 9 }
    ));

    let report = set.reconcile(&library);
    assert_eq!(
        stubbed(&report.main.repairs),
        [(n(2), DetachReason::TooDeep)]
    );
    assert!(report.subgraphs.is_empty());
    assert_eq!(set.main().len(), 1);
    set.validate_with(&library).unwrap();
}

/// 256 `src`s in one subgraph and 256 instances of it in another make 2¹⁶ program nodes, which
/// [`ProgramShape::MAX_NODES`] admits; a `src` beside an instance of the second in the main
/// graph makes one more, which it does not. Nodes are taken in id order, so the `src`, 1, stays
/// and the instance, 2, goes.
#[test]
fn a_program_past_the_node_bound_is_refused_and_detached() {
    let library = library();
    let mut set = GraphSet::default();
    set.insert_subgraph(s(1), empty("wide"));
    set.insert_subgraph(s(2), empty("wider"));
    for id in 0..256 {
        set.graph_mut(GraphId::Subgraph(s(1)))
            .insert(n(1000 + id), node(&library, SRC));
        place(&mut set, GraphId::Subgraph(s(2)), n(1000 + id), s(1));
    }
    set.main_mut().insert(n(1), node(&library, SRC));
    place(&mut set, GraphId::Main, n(2), s(2));

    let error = set.validate_with(&library).unwrap_err();
    assert_eq!(error.graph, GraphId::Main);
    assert!(matches!(
        error.error,
        GraphValidationError::TooLarge { node_count } if node_count == ProgramShape::MAX_NODES + 1
    ));

    let report = set.reconcile(&library);
    assert_eq!(
        stubbed(&report.main.repairs),
        [(n(2), DetachReason::TooLarge)]
    );
    assert!(set.main().find(n(1)).is_some());
    set.validate_with(&library).unwrap();
}

/// An add grows every graph that places the graph it lands in. Subgraph 1 holds 255 `src`s,
/// subgraph 2 holds 256 instances of it, 255 × 256 = 65 280 program nodes, and the main graph an
/// instance of 2 and 255 `src`s, 65 535. One `src` more inside 2 brings the main graph to
/// 65 536, the bound, and is admitted; two bring it to 65 537 and are refused, naming the main
/// graph and its count, though 2 itself stays within the bound.
#[test]
fn an_add_inside_a_subgraph_is_held_to_the_bound_of_each_graph_that_places_it() {
    let library = library();
    let mut set = GraphSet::default();
    set.insert_subgraph(s(1), empty("leaves"));
    set.insert_subgraph(s(2), empty("rows"));
    for id in 0..255 {
        set.graph_mut(GraphId::Subgraph(s(1)))
            .insert(n(1000 + id), node(&library, SRC));
        set.main_mut().insert(n(1000 + id), node(&library, SRC));
    }
    for id in 0..256 {
        place(&mut set, GraphId::Subgraph(s(2)), n(2000 + id), s(1));
    }
    place(&mut set, GraphId::Main, n(1), s(2));
    set.validate_with(&library).unwrap();

    let src = node(&library, SRC);
    assert_eq!(set.admit_nodes(GraphId::Subgraph(s(2)), [&src]), Ok(()));
    assert_eq!(
        set.admit_nodes(GraphId::Subgraph(s(2)), [&src, &src]),
        Err(PlacementRejection::TooLarge {
            graph: GraphId::Main,
            node_count: ProgramShape::MAX_NODES + 1
        })
    );
}

/// A subgraph's graph holds one boundary node a side. A load keeps the lowest id of two and adds
/// one where none is; a boundary node in the main graph, which has no interface, is kept as a
/// stub.
#[test]
fn a_load_keeps_one_boundary_node_a_side() {
    let library = library();
    let mut subgraph = Subgraph::new("edged", Interface::default());
    for boundary in [Boundary::Inputs, Boundary::Outputs] {
        subgraph.graph.detach_node(subgraph.boundary_node(boundary));
    }
    let (kept, extra) = (n(1), n(2));
    subgraph.graph.insert(kept, Node::from(Boundary::Inputs));
    subgraph.graph.insert(extra, Node::from(Boundary::Inputs));
    let mut set = GraphSet::default();
    set.insert_subgraph(s(1), subgraph);
    let in_subgraph = set.scope(&library, GraphId::Subgraph(s(1)));
    assert!(matches!(
        set.graph(GraphId::Subgraph(s(1)))
            .validate_with(in_subgraph),
        Err(GraphValidationError::BoundaryCount {
            boundary: Boundary::Inputs,
            count: 2,
        })
    ));
    set.main_mut().insert(n(5), Node::from(Boundary::Outputs));

    let error = set.validate_with(&library).unwrap_err();
    assert_eq!(error.graph, GraphId::Main);
    assert!(matches!(
        error.error,
        GraphValidationError::Unresolved {
            reason: Unresolved::NoInterface,
            ..
        }
    ));

    let report = set.reconcile(&library);
    assert_eq!(
        report.main.repairs,
        [Repair::Stubbed {
            node_id: n(5),
            reason: DetachReason::Unresolved(Unresolved::NoInterface),
        }]
    );
    let subgraph = set.subgraph(s(1)).unwrap();
    assert_eq!(subgraph.boundary_node(Boundary::Inputs), kept);
    let added = subgraph.boundary_node(Boundary::Outputs);
    assert_eq!(
        report.subgraphs[&s(1)].repairs,
        [
            Repair::BoundaryAdded {
                boundary: Boundary::Outputs,
                node_id: added,
            },
            Repair::Stubbed {
                node_id: extra,
                reason: DetachReason::ExtraBoundary,
            },
        ]
    );
    set.validate_with(&library).unwrap();
}

/// A load attaches each stub whose node resolves again, in any graph, with the wiring that still
/// has both ends and lands on an input the graph has not bound since, and drops a stub saved
/// under a node's id. Here, inside subgraph 1, the stub of `sink` 3 holds a wire from `src` 1,
/// which comes back, and a subscription to ticker 9, which the graph lacks, so it goes; in the
/// main graph, a stub saved under the id of `src` 4 goes and the node stays.
#[test]
fn a_load_attaches_each_stub_that_resolves_again_with_the_wiring_that_fits() {
    let library = library();
    let mut subgraph = empty("holder");
    subgraph.graph.insert(n(1), node(&library, SRC));
    let into_sink = InputPort::new(n(3), numbered_port(1));
    let from_ticker = Subscription::new(EventPort::new(n(9), numbered_port(1)), n(3));
    let record = |node_id, node, bindings, subscriptions| {
        DetachedNode::new(DetachedNodeParts {
            node_id,
            node,
            bindings,
            subscriptions,
        })
        .unwrap()
    };
    subgraph.graph.insert_stub(Stub {
        record: record(
            n(3),
            node(&library, SINK),
            vec![BindingEntry {
                port: into_sink,
                binding: Binding::wire(n(1), numbered_port(1)),
            }],
            vec![from_ticker],
        ),
        reason: DetachReason::Unresolved(Unresolved::MissingFunc),
    });
    let mut set = GraphSet::default();
    set.insert_subgraph(s(1), subgraph);
    let mut duplicate = set.main().clone();
    duplicate.insert(n(4), node(&library, SRC));
    set.main_mut().insert(n(4), node(&library, SRC));
    set.main_mut().stubs = BTreeMap::from([(
        n(4),
        Stub {
            record: record(n(4), node(&library, SRC), vec![], vec![]),
            reason: DetachReason::TooLarge,
        },
    )]);

    let report = set.reconcile(&library);
    assert_eq!(
        report.main.repairs,
        [Repair::DuplicateStub { node_id: n(4) }]
    );
    assert_eq!(*set.main(), duplicate);
    assert_eq!(
        report.subgraphs[&s(1)].repairs,
        [
            Repair::Reattached { node_id: n(3) },
            Repair::StubSubscriptionDropped {
                node_id: n(3),
                subscription: from_ticker,
            },
        ]
    );
    let graph = set.graph(GraphId::Subgraph(s(1)));
    assert_eq!(graph.stubs().len(), 0);
    assert_eq!(
        graph.bindings.get(&into_sink),
        Some(&Binding::wire(n(1), numbered_port(1)))
    );
    assert_eq!(graph.subscriptions().len(), 0);
    set.validate_with(&library).unwrap();
}

fn int_port(id: u128) -> InputDecl {
    InputDecl {
        id: PortId::from_u128(id),
        ..InputDecl::required("x", DataType::Int)
    }
}

/// The interface of `inputs` alone.
fn inputs(inputs: Vec<InputDecl>) -> Interface {
    Interface::new(Ports {
        inputs,
        outputs: vec![],
    })
}

/// An instance resolves to its subgraph's interface as it is now, and to nothing where the set
/// holds no subgraph under its id.
#[test]
fn an_instance_resolves_to_its_interface_as_it_is() {
    let library = library();
    let mut set = GraphSet::default();
    set.insert_subgraph(s(1), Subgraph::new("one", inputs(vec![int_port(7)])));
    place(&mut set, GraphId::Main, n(1), s(1));
    let name = |set: &GraphSet| {
        set.scope(&library, GraphId::Main)
            .resolve(set.main().find(n(1)).unwrap())
            .map(|decl| decl.inputs()[0].name.clone())
    };
    assert_eq!(name(&set), Ok("x".to_owned()));

    let renamed = inputs(vec![InputDecl {
        name: "renamed".to_owned(),
        ..int_port(7)
    }]);
    let change = set.plan_interface(s(1), renamed, &library);
    set.write_interface(&change, ChangeSide::After);
    assert_eq!(name(&set), Ok("renamed".to_owned()));

    let mut lacking = GraphSet::from(set.main().clone());
    lacking.main_mut().find_mut(n(1)).unwrap().name = "orphan".to_owned();
    assert_eq!(
        lacking
            .scope(&library, GraphId::Main)
            .resolve(lacking.main().find(n(1)).unwrap())
            .map(|decl| decl.name().to_owned()),
        Err(Unresolved::MissingSubgraph)
    );
}

/// The `Inputs` node shows each interface input as an output of its type, and the `Outputs` node
/// each interface output as an optional input. A port made from an `Any` input is an `Any`
/// output, which feeds an `Any` input and no `Int` one. An instance and a boundary node have no
/// body, so neither may subscribe to an event.
#[test]
fn boundary_nodes_mirror_the_interface_under_the_one_admission_rule() {
    let library = library();
    let interface = Interface::new(Ports {
        inputs: vec![InputDecl {
            id: PortId::from_u128(1),
            ..InputDecl::required("anything", DataType::Any)
        }],
        outputs: vec![OutputDecl {
            id: PortId::from_u128(2),
            ..OutputDecl::new("count", DataType::Int)
        }],
    });
    let mut subgraph = Subgraph::new("mirror", interface);
    let inputs = subgraph.boundary_node(Boundary::Inputs);
    let outputs = subgraph.boundary_node(Boundary::Outputs);
    let graph = &mut subgraph.graph;
    graph.insert(n(1), node(&library, ANY_SINK));
    graph.insert(n(2), node(&library, SINK));
    graph.insert(n(3), node(&library, TICKER));
    let mut set = GraphSet::default();
    set.insert_subgraph(s(1), subgraph);
    let scope = set.scope(&library, GraphId::Subgraph(s(1)));
    let graph = set.graph(GraphId::Subgraph(s(1)));

    let inputs_decl = graph.find(inputs).unwrap().decl(scope);
    assert!(inputs_decl.inputs().is_empty());
    assert_eq!(inputs_decl.outputs()[0].data_type, DataType::Any);
    let outputs_decl = graph.find(outputs).unwrap().decl(scope);
    assert!(outputs_decl.outputs().is_empty());
    assert!(!outputs_decl.inputs()[0].required);
    assert_eq!(outputs_decl.inputs()[0].data_type, DataType::Int);
    assert!(matches!(
        inputs_decl,
        NodeDecl::Boundary {
            side: Boundary::Inputs,
            ..
        }
    ));
    assert!(inputs_decl.func().is_none(), "a boundary node has no body");

    let any_port = PortId::from_u128(1);
    let from_inputs = Binding::Wire(OutputPort::new(inputs, any_port));
    assert_eq!(
        graph.admit(scope, InputPort::new(n(1), numbered_port(1)), &from_inputs),
        Ok(())
    );
    assert_eq!(
        graph.admit(scope, InputPort::new(n(2), numbered_port(1)), &from_inputs),
        Err(BindingRejection::WireMismatch)
    );
    assert_eq!(
        graph.admit(scope, InputPort::new(outputs, any_port), &from_inputs),
        Err(BindingRejection::UndeclaredInput)
    );

    assert_eq!(
        graph.admit_subscription(
            scope,
            Subscription::new(EventPort::new(n(3), numbered_port(1)), outputs)
        ),
        Err(SubscriptionRejection::BodilessSubscriber)
    );
    assert_eq!(
        graph.admit_subscription(
            scope,
            Subscription::new(EventPort::new(n(3), numbered_port(1)), n(1))
        ),
        Ok(())
    );
    assert_eq!(
        graph.admit_subscription(
            scope,
            Subscription::new(EventPort::new(inputs, any_port), n(1))
        ),
        Err(SubscriptionRejection::UndeclaredEvent),
        "an interface has no event ports, so a boundary node fires none"
    );
}

/// An instance seeds its inputs' declared defaults, as a func node does.
#[test]
fn a_new_instance_takes_its_interface_defaults() {
    let interface = inputs(vec![InputDecl {
        id: PortId::from_u128(1),
        ..InputDecl::optional("gain", DataType::Float).default(2.0)
    }]);
    let subgraph = Subgraph::new("gain", interface);
    let mut graph = Graph::default();
    let node_id = graph.add_instance(s(1), &subgraph);
    assert_eq!(
        graph.input_bindings(node_id).collect::<Vec<_>>(),
        [(
            InputPort::new(node_id, PortId::from_u128(1)),
            &Binding::Const(2.0.into())
        )]
    );
    let node = graph.find(node_id).unwrap();
    assert_eq!(node.subgraph(), Some(s(1)));
    assert_eq!(node.name, "gain");
    assert_eq!(node.cache(), None, "an instance has no cache mode");
}

/// A set round-trips with what its interfaces derive built again, and a file that names one
/// subgraph id twice is refused rather than loaded with the last.
#[test]
fn a_set_round_trips_and_a_repeated_subgraph_id_is_refused() {
    let mut set = GraphSet::default();
    set.insert_subgraph(s(1), Subgraph::new("one", inputs(vec![int_port(7)])));
    let ron = SerdeFormat::Ron.serialize(&set).unwrap();
    let back: GraphSet = SerdeFormat::Ron.deserialize(&ron).unwrap();
    assert_eq!(back, set);
    let interface = |set: &GraphSet| set.subgraph(s(1)).unwrap().interface.clone();
    assert_eq!(
        interface(&back).boundary_ports(Boundary::Inputs),
        interface(&set).boundary_ports(Boundary::Inputs)
    );

    let one = BTreeMap::from([(s(1), set.subgraph(s(1)).unwrap().clone())]);
    let text = String::from_utf8(SerdeFormat::Ron.serialize(&one).unwrap()).unwrap();
    let entry = text
        .trim()
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap()
        .trim()
        .trim_end_matches(',');
    let main = String::from_utf8(SerdeFormat::Ron.serialize(&Graph::default()).unwrap()).unwrap();
    let repeated = format!("(main: {main}, subgraphs: {{{entry},{entry}}})");
    let error = SerdeFormat::Ron
        .deserialize::<GraphSet>(repeated.as_bytes())
        .unwrap_err();
    let chain = ErrorChain::new(&error).to_string();
    assert!(chain.contains("duplicate subgraph id"), "{chain}");
}

/// A const-only input takes a wire from the `Inputs` node's mirror of a const-only interface
/// input, which an instance binds to a constant alone, and from nothing else: not from a plain
/// interface input, not from a func's output.
#[test]
fn a_const_only_input_takes_a_wire_from_a_const_only_interface_input_alone() {
    let library = library();
    let interface = inputs(vec![
        InputDecl {
            id: PortId::from_u128(1),
            ..InputDecl::required("config", DataType::Int).const_only()
        },
        int_port(2),
    ]);
    let mut subgraph = Subgraph::new("configured", interface);
    let inputs_node = subgraph.boundary_node(Boundary::Inputs);
    subgraph.graph.insert(n(1), node(&library, CONFIG_SINK));
    subgraph.graph.insert(n(2), node(&library, SRC));
    let mut set = GraphSet::default();
    set.insert_subgraph(s(1), subgraph);
    let scope = set.scope(&library, GraphId::Subgraph(s(1)));
    let graph = set.graph(GraphId::Subgraph(s(1)));
    let into_config = |producer: NodeId, port: u128| {
        graph.admit(
            scope,
            InputPort::new(n(1), numbered_port(1)),
            &Binding::Wire(OutputPort::new(producer, PortId::from_u128(port))),
        )
    };
    assert_eq!(into_config(inputs_node, 1), Ok(()));
    assert_eq!(
        into_config(inputs_node, 2),
        Err(BindingRejection::WiredConstOnly)
    );
    assert_eq!(into_config(n(2), 1), Err(BindingRejection::WiredConstOnly));
}

/// An instance path names the graph it ends in through each instance on it, and none once one
/// is not an instance; a graph's instances are every path that ends in it; a subgraph's uses are every node that places it, a stub's included, in
/// each graph; a subgraph no one uses is removed, and one in use refuses.
#[test]
fn paths_and_uses_read_the_instances_of_every_graph() {
    let library = library();
    let mut set = GraphSet::default();
    set.insert_subgraph(s(1), empty("inner"));
    set.insert_subgraph(s(2), empty("outer"));
    set.insert_subgraph(s(3), empty("unused"));
    place(&mut set, GraphId::Subgraph(s(2)), n(20), s(1));
    place(&mut set, GraphId::Main, n(2), s(2));
    place(&mut set, GraphId::Main, n(3), s(1));
    set.main_mut().insert(n(4), node(&library, SRC));
    let stubbed = set.main_mut().detach_node(n(3));
    set.main_mut().insert_stub(Stub {
        record: stubbed,
        reason: DetachReason::TooLarge,
    });

    let path = |ids: &[u128]| InstancePath::new(&ids.iter().copied().map(n).collect::<Vec<_>>());
    assert_eq!(set.graph_at(InstancePath::MAIN), Some(GraphId::Main));
    assert_eq!(set.graph_at(path(&[2])), Some(GraphId::Subgraph(s(2))));
    assert_eq!(set.graph_at(path(&[2, 20])), Some(GraphId::Subgraph(s(1))));
    assert_eq!(set.graph_at(path(&[4])), None, "a `src` places nothing");
    assert_eq!(set.graph_at(path(&[3])), None, "a stub places nothing");
    assert_eq!(set.graph_at(path(&[9])), None);

    assert_eq!(set.instances(GraphId::Main), [InstancePath::MAIN]);
    // `s(1)` is placed in the main graph by the stub `n(3)` alone, which expands nothing.
    assert_eq!(set.instances(GraphId::Subgraph(s(1))), [path(&[2, 20])]);
    assert_eq!(set.instances(GraphId::Subgraph(s(2))), [path(&[2])]);
    assert_eq!(set.instances(GraphId::Subgraph(s(3))), []);
    place(&mut set, GraphId::Main, n(1), s(2));
    assert_eq!(
        set.instances(GraphId::Subgraph(s(1))),
        [path(&[1, 20]), path(&[2, 20])],
        "each instance of the graph that places it, in path order"
    );
    set.main_mut().detach_node(n(1));

    assert_eq!(
        set.uses(s(1)).collect::<Vec<_>>(),
        [
            NodeAddr::new(GraphId::Main, n(3)),
            NodeAddr::new(GraphId::Subgraph(s(2)), n(20)),
        ]
    );
    assert_eq!(
        set.uses(s(2)).collect::<Vec<_>>(),
        [NodeAddr::new(GraphId::Main, n(2))]
    );
    assert_eq!(set.uses(s(3)).count(), 0);

    assert!(set.remove_subgraph(s(3)).is_some());
    assert!(set.subgraph(s(3)).is_none());
    let in_use = panic::catch_unwind(AssertUnwindSafe(|| set.remove_subgraph(s(1))));
    assert!(in_use.is_err(), "a placed subgraph is not removed");
}

/// Each type an interface names that the library does not register is reported once, with every
/// port that names it, in subgraph order, inputs before outputs; a registered type is not.
#[test]
fn unknown_types_name_each_interface_port_that_uses_them() {
    let library = library();
    let unknown = DataType::Custom(CustomTypeId::from_u128(77));
    let port = |id: u128, data_type| InputDecl {
        id: PortId::from_u128(id),
        ..InputDecl::required("x", data_type)
    };
    let mut set = GraphSet::default();
    set.insert_subgraph(
        s(1),
        Subgraph::new(
            "one",
            Interface::new(Ports {
                inputs: vec![port(1, DataType::Int), port(2, unknown)],
                outputs: vec![OutputDecl {
                    id: PortId::from_u128(3),
                    ..OutputDecl::new("y", unknown)
                }],
            }),
        ),
    );
    set.insert_subgraph(s(2), Subgraph::new("two", inputs(vec![port(4, unknown)])));
    let at = |subgraph, list, id: u128| InterfacePort {
        subgraph,
        list,
        port: PortId::from_u128(id),
    };
    assert_eq!(
        set.unknown_types(&library),
        [UnknownType {
            data_type: unknown,
            ports: vec![
                at(s(1), PortList::Inputs, 2),
                at(s(1), PortList::Outputs, 3),
                at(s(2), PortList::Inputs, 4),
            ],
        }]
    );
}

const A: PortId = PortId::from_u128(101);
const B: PortId = PortId::from_u128(102);
const SUM: PortId = PortId::from_u128(201);

/// `adder`'s interface: `a` and `b` of `a_type` and `b_type`, and an `Int` `sum`.
fn adder_interface(a_type: DataType, b_type: DataType) -> Interface {
    Interface::new(Ports {
        inputs: vec![
            InputDecl {
                id: A,
                ..InputDecl::required("a", a_type)
            },
            InputDecl {
                id: B,
                ..InputDecl::required("b", b_type)
            },
        ],
        outputs: vec![OutputDecl {
            id: SUM,
            ..OutputDecl::new("sum", DataType::Int)
        }],
    })
}

/// Subgraph 1, `adder`: its `Inputs` node's `a` and `b` into `add` 13, whose sum is the `Outputs`
/// node's. The main graph: `src` 1 into instance 2's `a`, a constant 7 on its `b`, and `sink` 3
/// reading its sum. So each interface port is bound in two graphs.
fn adder_set() -> (GraphSet, Library) {
    let library = library();
    let mut adder = Subgraph::new("adder", adder_interface(DataType::Int, DataType::Int));
    let (inputs, outputs) = (
        adder.boundary_node(Boundary::Inputs),
        adder.boundary_node(Boundary::Outputs),
    );
    adder.graph.insert(n(13), node(&library, ADD));
    let inner = &mut adder.graph;
    inner.set_input_binding(
        InputPort::new(n(13), numbered_port(1)),
        Binding::wire(inputs, A),
    );
    inner.set_input_binding(
        InputPort::new(n(13), numbered_port(2)),
        Binding::wire(inputs, B),
    );
    inner.set_input_binding(
        InputPort::new(outputs, SUM),
        Binding::wire(n(13), numbered_port(1)),
    );
    let mut set = GraphSet::default();
    set.insert_subgraph(s(1), adder);
    set.main_mut().insert(n(1), node(&library, SRC));
    place(&mut set, GraphId::Main, n(2), s(1));
    set.main_mut().insert(n(3), node(&library, SINK));
    let main = set.main_mut();
    main.set_input_binding(
        InputPort::new(n(2), A),
        Binding::wire(n(1), numbered_port(1)),
    );
    main.set_input_binding(InputPort::new(n(2), B), Binding::Const(7i64.into()));
    main.set_input_binding(
        InputPort::new(n(3), numbered_port(1)),
        Binding::wire(n(2), SUM),
    );
    set.validate_with(&library).unwrap();
    (set, library)
}

/// A reorder and a rename change no binding in any graph: the plan drops nothing, and the set
/// after differs from the one before in the interface alone.
#[test]
fn an_interface_reorder_and_rename_drop_nothing() {
    let (mut set, library) = adder_set();
    let before = set.clone();
    let [a, b] = adder_interface(DataType::Int, DataType::Int)
        .inputs()
        .to_owned()
        .try_into()
        .unwrap();
    let reordered = Interface::new(Ports {
        inputs: vec![
            b,
            InputDecl {
                name: "alpha".to_owned(),
                ..a
            },
        ],
        outputs: adder_interface(DataType::Int, DataType::Int)
            .outputs()
            .to_vec(),
    });
    let change = set.plan_interface(s(1), reordered.clone(), &library);
    assert!(change.dropped().is_empty());
    set.write_interface(&change, ChangeSide::After);
    assert_eq!(set.subgraph(s(1)).unwrap().interface, reordered);
    for (graph, held) in before.graphs() {
        assert_eq!(set.graph(graph), held, "{graph:?} keeps every binding");
    }
    set.validate_with(&library).unwrap();
}

/// A port removed takes its wiring in both graphs and nothing else: input `a` its wire on the
/// instance and its wire from the `Inputs` node, output `sum` the sink's wire from the instance
/// and the `Outputs` node's wire from `add`. The write lands it, and the backward write puts all
/// four back, leaving the set as it was.
#[test]
fn an_interface_port_removed_drops_its_wires_in_every_graph_and_undo_restores_them() {
    let (mut set, library) = adder_set();
    let before = set.clone();
    let [_, b] = adder_interface(DataType::Int, DataType::Int)
        .inputs()
        .to_owned()
        .try_into()
        .unwrap();
    let without = Interface::new(Ports {
        inputs: vec![b],
        outputs: vec![],
    });
    let adder = set.subgraph(s(1)).unwrap();
    let (inputs, outputs) = (
        adder.boundary_node(Boundary::Inputs),
        adder.boundary_node(Boundary::Outputs),
    );
    let change = set.plan_interface(s(1), without.clone(), &library);
    let dropped = |graph, port, binding| DroppedBinding {
        graph,
        port,
        binding,
    };
    let inside = GraphId::Subgraph(s(1));
    let mut expected = vec![
        dropped(
            GraphId::Main,
            InputPort::new(n(2), A),
            Binding::wire(n(1), numbered_port(1)),
        ),
        dropped(
            GraphId::Main,
            InputPort::new(n(3), numbered_port(1)),
            Binding::wire(n(2), SUM),
        ),
        dropped(
            inside,
            InputPort::new(n(13), numbered_port(1)),
            Binding::wire(inputs, A),
        ),
        dropped(
            inside,
            InputPort::new(outputs, SUM),
            Binding::wire(n(13), numbered_port(1)),
        ),
    ];
    // Graph by graph, each in the order the graph keeps its bindings.
    expected.sort_by_key(|dropped| (dropped.graph, dropped.port));
    assert_eq!(change.dropped(), expected);
    assert_eq!(change.before(), &before.subgraph(s(1)).unwrap().interface);
    assert_eq!(change.after(), &without);

    set.write_interface(&change, ChangeSide::After);
    assert!(!set.main().bindings.contains_key(&InputPort::new(n(2), A)));
    set.validate_with(&library).unwrap();
    set.write_interface(&change, ChangeSide::Before);
    assert_eq!(set, before);
}

/// A port retyped drops only what the new type refuses: `b` as a `String` refuses the constant 7
/// on the instance and its wire into the `Int` input of `add`, and keeps every binding of `a`.
#[test]
fn an_interface_port_retyped_drops_only_what_the_new_type_refuses() {
    let (mut set, library) = adder_set();
    let inputs = set.subgraph(s(1)).unwrap().boundary_node(Boundary::Inputs);
    let change = set.plan_interface(
        s(1),
        adder_interface(DataType::Int, DataType::String),
        &library,
    );
    assert_eq!(
        change.dropped(),
        [
            DroppedBinding {
                graph: GraphId::Main,
                port: InputPort::new(n(2), B),
                binding: Binding::Const(7i64.into()),
            },
            DroppedBinding {
                graph: GraphId::Subgraph(s(1)),
                port: InputPort::new(n(13), numbered_port(2)),
                binding: Binding::wire(inputs, B),
            },
        ]
    );
    set.write_interface(&change, ChangeSide::After);
    set.validate_with(&library).unwrap();
}

/// A node of a func the library lacks is kept as a stub, stays one at each load that still lacks
/// it, and is attached again by the first load whose library holds it.
#[test]
fn a_stub_of_a_missing_func_stays_one_until_the_library_holds_it() {
    let mut library = library();
    let missing = FuncId::from_u128(99);
    let mut set = GraphSet::default();
    set.main_mut()
        .insert(n(1), internals::unresolved_node(missing));
    assert_eq!(
        stubbed(&set.reconcile(&library).main.repairs),
        [(n(1), DetachReason::Unresolved(Unresolved::MissingFunc))]
    );
    assert!(
        set.reconcile(&library).is_empty(),
        "a stub that still does not resolve stays"
    );

    library.add(internals::stub_func(missing, "found").build());
    assert_eq!(
        set.reconcile(&library).main.repairs,
        [Repair::Reattached { node_id: n(1) }]
    );
    assert!(set.main().find(n(1)).is_some());
    assert_eq!(set.main().stubs().len(), 0);
}

/// A digest is of the definition alone: two sets holding one definition under two ids, its
/// nodes added in two orders and its placed subgraph under two ids, digest it alike; a renamed
/// inner node, or a changed placed subgraph, changes it.
#[test]
fn a_digest_names_the_definition_and_not_its_ids() {
    let library = library();
    // A definition whose nodes, the boundary nodes among them, go in under fixed ids in `order`.
    let definition = |name: &str, order: &[(u128, Node)]| {
        let mut graph = Graph::default();
        for (id, held) in order {
            graph.insert(n(*id), held.clone());
        }
        Subgraph {
            name: name.to_string(),
            description: None,
            interface: Interface::default(),
            graph,
        }
    };
    let build = |outer: SubgraphId, inner: SubgraphId, reversed: bool, inner_name: &str| {
        let mut set = GraphSet::default();
        let placed = definition(
            inner_name,
            &[
                (1, Node::from(Boundary::Inputs)),
                (2, Node::from(Boundary::Outputs)),
                (5, node(&library, SRC)),
            ],
        );
        let instance = Node::instance(inner, &placed);
        set.insert_subgraph(inner, placed);
        let mut order = vec![
            (1, Node::from(Boundary::Inputs)),
            (2, Node::from(Boundary::Outputs)),
            (10, node(&library, SRC)),
            (11, node(&library, SINK)),
            (9, instance),
        ];
        if reversed {
            order.reverse();
        }
        set.insert_subgraph(outer, definition("outer", &order));
        set
    };
    let one = build(s(1), s(2), false, "inner");
    let two = build(s(7), s(8), true, "inner");
    assert_eq!(one.digest(s(1)), two.digest(s(7)));
    assert_eq!(one.digest(s(2)), two.digest(s(8)));

    let mut renamed = build(s(1), s(2), false, "inner");
    renamed
        .graph_mut(GraphId::Subgraph(s(1)))
        .find_mut(n(10))
        .unwrap()
        .name = "renamed".to_string();
    assert_ne!(
        renamed.digest(s(1)),
        one.digest(s(1)),
        "an inner rename changes it"
    );
    let deeper = build(s(1), s(2), false, "other inner");
    assert_ne!(deeper.digest(s(2)), one.digest(s(2)));
    assert_ne!(
        deeper.digest(s(1)),
        one.digest(s(1)),
        "a placed subgraph's change reaches the one that places it"
    );
}
