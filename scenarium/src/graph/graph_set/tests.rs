use std::collections::BTreeMap;
use std::panic;
use std::panic::AssertUnwindSafe;

use common::{ErrorChain, SerdeFormat};

use crate::data::data_type::DataType;
use crate::data::data_type::custom_type::{CustomType, CustomTypeId};
use crate::data::data_type::enum_type::{EnumId, EnumType};
use crate::data::data_type::type_ref::TypeRef;
use crate::decl::cache_mode::CacheMode;
use crate::decl::ports::error::BindingRejection;
use crate::decl::ports::error::PortList;
use crate::decl::ports::{InputDecl, Numbered, OutputDecl, Ports};
use crate::decl::saved_decl::{SavedDecl, SavedPort};
use crate::graph::binding::Binding;
use crate::graph::compound::{Compound, CompoundDecl};
use crate::graph::error::{
    DetachReason, GraphValidationError, PlacementRejection, SubscriptionRejection, Unresolved,
};
use crate::graph::ghost_node::GhostNode;
use crate::graph::ghost_wiring::{GhostBinding, GhostSource, InputState};
use crate::graph::graph_set::GraphSet;
use crate::graph::graph_set::program_shape::ProgramShape;
use crate::graph::graph_set::unknown_type::{InterfacePort, UnknownType};
use crate::graph::interface::Interface;
use crate::graph::interface_change::InterfaceChange;
use crate::graph::interface_change::{ChangeSide, RefusedFate};
use crate::graph::manifest::Manifest;
use crate::graph::node::{Boundary, Node};
use crate::graph::node_decl::NodeDecl;
use crate::graph::reconciled::Repair;
use crate::graph::subscription::Subscription;
use crate::graph::{Detaching, Graph};
use crate::identity::instance_path::InstancePath;
use crate::identity::{
    CompoundId, EventPort, FuncId, GraphId, InputPort, NodeAddr, NodeId, OutputPort, PortId,
};
use crate::internals;
use crate::internals::test_graph::sets::{
    ADD, ANY_SINK, CONFIG_SINK, SINK, SRC, TICKER, library, n, node, s,
};
use crate::library::Library;

/// A compound with no ports.
fn empty(name: &str) -> Compound {
    Compound::new(name, Interface::default())
}

/// A node with no name of its own reads its declaration's, a ghost node its saved one's: a func
/// renamed in the library and a renamed compound each read renamed on an unnamed node, and a node
/// named keeps its name.
#[test]
fn an_unnamed_node_reads_its_declarations_name() {
    let library = library();
    let mut set = GraphSet::default();
    set.insert_compound(s(1), empty("first"));
    set.main_mut().insert(n(1), node(&library, SRC));
    place(&mut set, GraphId::Main, n(2), s(1));
    let mut named = node(&library, SRC);
    named.name = Some("mine".to_owned());
    set.main_mut().insert(n(3), named);
    let titles = |set: &GraphSet, library: &Library| {
        let scope = set.scope(library, GraphId::Main);
        [n(1), n(2), n(3)].map(|node_id| scope.title(set.main().find(node_id).unwrap()).to_owned())
    };
    assert_eq!(titles(&set, &library), ["src", "first", "mine"]);

    let mut renamed = Library::default();
    renamed.add(
        internals::stub_func(SRC, "source")
            .pure()
            .output(1, OutputDecl::new("value", DataType::Int))
            .build(),
    );
    set.rename_compound(s(1), "second".to_owned());
    assert_eq!(titles(&set, &renamed), ["source", "second", "mine"]);

    let ghost = |name: Option<&str>, saved: Option<&str>| GhostNode {
        node: Node {
            name: name.map(str::to_owned),
            ..node(&library, SRC)
        },
        reason: DetachReason::Recursive,
        saved: saved.map(|name| SavedDecl {
            name: name.to_owned(),
            ..SavedDecl::default()
        }),
    };
    assert_eq!(ghost(Some("mine"), Some("src")).title(), "mine");
    assert_eq!(ghost(None, Some("src")).title(), "src");
    assert_eq!(ghost(None, None).title(), "");
}

/// Put an instance of `placed` into `graph` under `node_id`.
fn place(set: &mut GraphSet, graph: GraphId, node_id: NodeId, placed: CompoundId) {
    let node = Node::instance(placed);
    set.graph_mut(graph).insert(node_id, node);
}

/// The repairs that made a ghost node of a node, as `(node, reason)`.
fn ghosted(repairs: &[Repair]) -> Vec<(NodeId, DetachReason)> {
    repairs
        .iter()
        .filter_map(|repair| match repair {
            Repair::GhostedNode { node_id, reason } => Some((*node_id, *reason)),
            _ => None,
        })
        .collect()
}

/// Compound 1 places 2, and 2 places nothing: a loop closes only by placing 1 inside 2 or
/// itself, and the main graph never closes one.
#[test]
fn a_placement_that_closes_a_loop_is_refused() {
    let mut set = GraphSet::default();
    set.insert_compound(s(1), empty("outer"));
    set.insert_compound(s(2), empty("inner"));
    place(&mut set, GraphId::Compound(s(1)), n(10), s(2));
    let instance = |placed| Node::instance(placed);
    let admit = |graph, placed| set.admit_nodes(graph, [&instance(placed)]);

    assert_eq!(
        admit(GraphId::Compound(s(2)), s(1)),
        Err(PlacementRejection::Recursive)
    );
    assert_eq!(
        admit(GraphId::Compound(s(1)), s(1)),
        Err(PlacementRejection::Recursive)
    );
    assert_eq!(
        admit(GraphId::Compound(s(2)), s(2)),
        Err(PlacementRejection::Recursive)
    );
    assert_eq!(admit(GraphId::Compound(s(1)), s(2)), Ok(()));
    assert_eq!(admit(GraphId::Main, s(1)), Ok(()));

    // Compound 1 places nothing yet, so a walk in id order reaches it before 2: an instance of 2
    // added to it is measured all the same.
    let mut apart = GraphSet::default();
    apart.insert_compound(s(1), empty("first"));
    apart.insert_compound(s(2), empty("second"));
    let second = Node::instance(s(2));
    assert_eq!(
        apart.admit_nodes(GraphId::Compound(s(1)), [&second]),
        Ok(())
    );
}

/// A set where 1 places 2 and 2 places 1 is refused, naming the instance a walk from the lowest
/// id meets on the path: 2's, as the walk enters 1, then 2, and returns to 1. A load keeps
/// exactly that one as a ghost node, after which the set validates; a second load attaches it, finds
/// the same loop, and keeps it as it was, so it reports nothing.
#[test]
fn a_loop_is_refused_and_a_load_breaks_it_at_one_instance() {
    let library = library();
    let mut set = GraphSet::default();
    set.insert_compound(s(1), empty("one"));
    set.insert_compound(s(2), empty("two"));
    place(&mut set, GraphId::Compound(s(1)), n(12), s(2));
    place(&mut set, GraphId::Compound(s(2)), n(21), s(1));

    let error = set.validate_with(&library).unwrap_err();
    assert_eq!(error.graph, GraphId::Compound(s(2)));
    assert!(matches!(
        error.error,
        GraphValidationError::Recursive { node_id } if node_id == n(21)
    ));

    let report = set.reconcile(&library, &Manifest::default());
    assert!(report.of(GraphId::Main).is_empty());
    assert_eq!(
        report.repairs.keys().copied().collect::<Vec<_>>(),
        [GraphId::Compound(s(2))]
    );
    assert_eq!(
        report.of(GraphId::Compound(s(2))),
        [Repair::GhostedNode {
            node_id: n(21),
            reason: DetachReason::Recursive,
        }]
    );
    let two = set.graph(GraphId::Compound(s(2)));
    assert_eq!(
        two.ghost_node(n(21)).unwrap().reason,
        DetachReason::Recursive
    );
    assert!(two.find(n(21)).is_none());
    assert!(set.graph(GraphId::Compound(s(1))).find(n(12)).is_some());
    set.validate_with(&library).unwrap();

    let reloaded = set.clone();
    assert!(set.reconcile(&library, &Manifest::default()).is_empty());
    assert_eq!(set, reloaded);

    // The broken loop is held by a ghost node, which a digest encodes by id rather than
    // follow, where the walk once went round the loop until the stack ran out. Each digest is
    // its own definition's, and the same each time.
    let (one, two) = (set.digest(s(1)), set.digest(s(2)));
    assert_ne!(one, two);
    assert_eq!(set.digest(s(1)), one);
    assert_eq!(set.placed_first(s(1)), [s(2), s(1)]);
}

/// A compound that places itself live, as a library file no load reconciled may hold, ends the
/// walks a copy and a digest take, where each once recursed until the stack ran out.
#[test]
fn a_compound_that_places_itself_ends_every_walk() {
    let mut set = GraphSet::default();
    set.insert_compound(s(1), empty("one"));
    place(&mut set, GraphId::Compound(s(1)), n(11), s(1));

    assert_eq!(set.placed_first(s(1)), [s(1)]);
    assert_eq!(set.digest(s(1)), set.digest(s(1)));
}

/// A compound under the nil id is refused at decode: no set holds one, and it has no graph a
/// load could repair it in.
#[test]
fn a_compound_under_the_nil_id_is_refused() {
    let mut set = GraphSet::default();
    set.insert_compound(s(1), empty("one"));
    let text = String::from_utf8(SerdeFormat::Ron.serialize(&set).unwrap()).unwrap();
    assert!(
        SerdeFormat::Ron
            .deserialize::<GraphSet>(text.as_bytes())
            .is_ok()
    );

    let nil = text.replace(&s(1).to_string(), &CompoundId::nil().to_string());
    assert_ne!(nil, text, "the id is spelled in the text");
    let error = SerdeFormat::Ron
        .deserialize::<GraphSet>(nil.as_bytes())
        .expect_err("a nil compound does not load");
    assert!(
        ErrorChain::new(&error)
            .to_string()
            .contains("a compound under the nil id"),
        "{}",
        ErrorChain::new(&error)
    );
}

/// A chain of eight compounds, each placing the next and the last holding a `src`, nests eight
/// levels: the last is 1, each one up adds one. The main graph placing the first would nest
/// nine, past [`ProgramShape::MAX_LEVELS`], so the gate refuses the add and a load keeps that
/// one instance as a ghost node, keeping the chain itself.
#[test]
fn a_ninth_level_is_refused_and_detached() {
    let library = library();
    let mut set = GraphSet::default();
    for id in 1..=8 {
        set.insert_compound(s(id), empty("link"));
    }
    for id in 1..8 {
        place(&mut set, GraphId::Compound(s(id)), n(100 + id), s(id + 1));
    }
    set.graph_mut(GraphId::Compound(s(8)))
        .insert(n(1), node(&library, SRC));
    set.main_mut().insert(n(1), node(&library, SRC));
    let first = Node::instance(s(1));
    assert_eq!(
        set.admit_nodes(GraphId::Main, [&first]),
        Err(PlacementRejection::TooDeep {
            graph: GraphId::Main,
            levels: 9
        })
    );
    let second = Node::instance(s(2));
    assert_eq!(set.admit_nodes(GraphId::Main, [&second]), Ok(()));

    set.main_mut().insert(n(2), first);
    let error = set.validate_with(&library).unwrap_err();
    assert_eq!(error.graph, GraphId::Main);
    assert!(matches!(
        error.error,
        GraphValidationError::Placement(PlacementRejection::TooDeep {
            graph: GraphId::Main,
            levels: 9
        })
    ));

    let report = set.reconcile(&library, &Manifest::default());
    assert_eq!(
        ghosted(&report.of(GraphId::Main)),
        [(n(2), DetachReason::TooDeep)]
    );
    assert!(report.repairs.keys().all(|&graph| graph == GraphId::Main));
    assert_eq!(set.main().len(), 1);
    set.validate_with(&library).unwrap();
}

/// 256 `src`s in one compound and 256 instances of it in another make 2¹⁶ program nodes, which
/// [`ProgramShape::MAX_NODES`] admits; a `src` beside an instance of the second in the main
/// graph makes one more, which it does not. Nodes are taken in id order, so the `src`, 1, stays
/// and the instance, 2, goes.
#[test]
fn a_program_past_the_node_bound_is_refused_and_detached() {
    let library = library();
    let mut set = GraphSet::default();
    set.insert_compound(s(1), empty("wide"));
    set.insert_compound(s(2), empty("wider"));
    for id in 0..256 {
        set.graph_mut(GraphId::Compound(s(1)))
            .insert(n(1000 + id), node(&library, SRC));
        place(&mut set, GraphId::Compound(s(2)), n(1000 + id), s(1));
    }
    set.main_mut().insert(n(1), node(&library, SRC));
    place(&mut set, GraphId::Main, n(2), s(2));

    let error = set.validate_with(&library).unwrap_err();
    assert_eq!(error.graph, GraphId::Main);
    assert!(matches!(
        error.error,
        GraphValidationError::Placement(PlacementRejection::TooLarge {
            graph: GraphId::Main,
            node_count,
        }) if node_count == ProgramShape::MAX_NODES + 1
    ));

    let report = set.reconcile(&library, &Manifest::default());
    assert_eq!(
        ghosted(&report.of(GraphId::Main)),
        [(n(2), DetachReason::TooLarge)]
    );
    assert!(set.main().find(n(1)).is_some());
    set.validate_with(&library).unwrap();
}

/// An add grows every graph that places the graph it lands in. Compound 1 holds 255 `src`s,
/// compound 2 holds 256 instances of it, 255 × 256 = 65 280 program nodes, and the main graph an
/// instance of 2 and 255 `src`s, 65 535. One `src` more inside 2 brings the main graph to
/// 65 536, the bound, and is admitted; two bring it to 65 537 and are refused, naming the main
/// graph and its count, though 2 itself stays within the bound.
#[test]
fn an_add_inside_a_compound_is_held_to_the_bound_of_each_graph_that_places_it() {
    let library = library();
    let mut set = GraphSet::default();
    set.insert_compound(s(1), empty("leaves"));
    set.insert_compound(s(2), empty("rows"));
    for id in 0..255 {
        set.graph_mut(GraphId::Compound(s(1)))
            .insert(n(1000 + id), node(&library, SRC));
        set.main_mut().insert(n(1000 + id), node(&library, SRC));
    }
    for id in 0..256 {
        place(&mut set, GraphId::Compound(s(2)), n(2000 + id), s(1));
    }
    place(&mut set, GraphId::Main, n(1), s(2));
    set.validate_with(&library).unwrap();

    let src = node(&library, SRC);
    assert_eq!(set.admit_nodes(GraphId::Compound(s(2)), [&src]), Ok(()));
    assert_eq!(
        set.admit_nodes(GraphId::Compound(s(2)), [&src, &src]),
        Err(PlacementRejection::TooLarge {
            graph: GraphId::Main,
            node_count: ProgramShape::MAX_NODES + 1
        })
    );
    // Two more in `leaves` pass the bound in both graphs that place it: `rows` expands into
    // 256 × 257 = 65 792 nodes and the main graph into 255 more. The main graph, first in id
    // order, is the one named.
    assert_eq!(
        set.admit_nodes(GraphId::Compound(s(1)), [&src, &src]),
        Err(PlacementRejection::TooLarge {
            graph: GraphId::Main,
            node_count: 256 * 257 + 255
        })
    );
}

/// A compound put under an id comes in beside the others, or in place of the one held there,
/// handing back what it replaced; a replacement keeps the interface, or panics with the set as it
/// was.
#[test]
fn a_compound_is_put_beside_or_in_place_of_one_held() {
    let mut set = GraphSet::default();
    assert_eq!(set.put_compound(s(1), empty("first")), None);
    assert_eq!(
        set.put_compound(s(1), empty("second"))
            .map(|held| held.decl.name),
        Some("first".to_owned())
    );
    assert_eq!(
        set.replace_compound(s(1), empty("third")).decl.name,
        "second"
    );
    assert_eq!(set.compound(s(1)).unwrap().decl.name, "third");

    let before = set.clone();
    let retyped = Compound::new("fourth", adder_interface(DataType::Int, DataType::Int));
    let refused = panic::catch_unwind(AssertUnwindSafe(|| {
        set.replace_compound(s(1), retyped);
    }));
    assert!(refused.is_err(), "another interface is no replacement");
    assert_eq!(set, before);
}

/// A compound's graph holds one `Outputs` node and one or more `Inputs` nodes. A load keeps both
/// `Inputs` nodes it finds, keeps the lowest id of two `Outputs` nodes and adds an `Inputs` node
/// where none is; a boundary node in the main graph, which has no interface, is kept as a ghost
/// node.
#[test]
fn a_load_keeps_every_inputs_node_and_one_outputs_node() {
    let library = library();
    let boundaries = |compound: &Compound| -> Vec<NodeId> {
        compound.graph.iter().map(|node| node.node_id).collect()
    };
    let mut two_inputs = Compound::new("two", Interface::default());
    for node_id in boundaries(&two_inputs) {
        two_inputs.graph.detach_node(node_id);
    }
    two_inputs.graph.insert(n(1), Node::from(Boundary::Inputs));
    two_inputs.graph.insert(n(2), Node::from(Boundary::Inputs));
    two_inputs.graph.insert(n(3), Node::from(Boundary::Outputs));
    two_inputs.graph.insert(n(4), Node::from(Boundary::Outputs));
    let mut no_inputs = Compound::new("none", Interface::default());
    no_inputs.graph.detach_node(no_inputs.graph.inputs_node());
    let mut set = GraphSet::default();
    set.insert_compound(s(1), two_inputs);
    set.insert_compound(s(2), no_inputs);
    let check = |set: &GraphSet, id| {
        set.graph(GraphId::Compound(id))
            .validate_with(set.scope(&library, GraphId::Compound(id)))
    };
    assert!(matches!(
        check(&set, s(1)),
        Err(GraphValidationError::OutputsCount { count: 2 })
    ));
    assert!(matches!(
        check(&set, s(2)),
        Err(GraphValidationError::NoInputsNode)
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

    let report = set.reconcile(&library, &Manifest::default());
    assert_eq!(
        report.of(GraphId::Main),
        [Repair::GhostedNode {
            node_id: n(5),
            reason: DetachReason::Unresolved(Unresolved::NoInterface),
        }]
    );
    let two = set.compound(s(1)).unwrap();
    let mut inputs: Vec<NodeId> = two.graph.boundary_nodes(Boundary::Inputs).collect();
    inputs.sort_unstable();
    assert_eq!(inputs, [n(1), n(2)]);
    assert_eq!(two.graph.inputs_node(), n(1));
    assert_eq!(two.graph.outputs_node(), n(3));
    assert_eq!(
        report.of(GraphId::Compound(s(1))),
        [Repair::GhostedNode {
            node_id: n(4),
            reason: DetachReason::ExtraOutputs,
        }]
    );
    let added = set.compound(s(2)).unwrap().graph.inputs_node();
    assert_eq!(
        report.of(GraphId::Compound(s(2))),
        [Repair::BoundaryAdded {
            boundary: Boundary::Inputs,
            node_id: added,
        }]
    );
    set.validate_with(&library).unwrap();
}

/// A load attaches each ghost node that resolves again, in any graph, and makes live the ghosts
/// the graph can hold again; a ghost node saved under a node's id goes. Here, inside compound 1,
/// the ghost node of `sink` 3 has a ghost wire from `src` 1, which comes back with it; in the
/// main graph, a ghost node saved under the id of `src` 4 goes and the node stays.
#[test]
fn a_load_attaches_each_ghost_node_that_resolves_again_with_its_wiring() {
    let library = library();
    let mut compound = empty("holder");
    compound.graph.insert(n(1), node(&library, SRC));
    let into_sink = InputPort::new(n(3), PortId::new(1));
    let from_src = Binding::wire(n(1), PortId::new(1));
    compound.graph.insert(n(3), node(&library, SINK));
    compound
        .graph
        .set_input_binding(into_sink, from_src.clone());
    let attached = compound.graph.clone();
    compound.graph.detach_into_ghosts(
        vec![Detaching {
            node_id: n(3),
            reason: DetachReason::Unresolved(Unresolved::MissingFunc),
            saved: None,
        }],
        &mut Vec::new(),
    );
    let mut set = GraphSet::default();
    set.insert_compound(s(1), compound);
    set.main_mut().insert(n(4), node(&library, SRC));
    let duplicate = set.main().clone();
    set.main_mut().ghost_nodes.insert(
        n(4),
        GhostNode {
            node: node(&library, SRC),
            reason: DetachReason::TooLarge,
            saved: None,
        },
    );

    let report = set.reconcile(&library, &Manifest::default());
    assert_eq!(
        report.of(GraphId::Main),
        [Repair::DuplicateGhostNode { node_id: n(4) }]
    );
    assert_eq!(*set.main(), duplicate);
    assert_eq!(
        report.of(GraphId::Compound(s(1))),
        [
            Repair::Reattached { node_id: n(3) },
            Repair::RestoredBinding {
                port: into_sink,
                binding: from_src,
            },
        ]
    );
    assert_eq!(*set.graph(GraphId::Compound(s(1))), attached);
    set.validate_with(&library).unwrap();
}

fn int_port(id: u32) -> Numbered<InputDecl> {
    Numbered::new(PortId::new(id), InputDecl::required("x", DataType::Int))
}

/// The interface of `inputs` alone.
fn inputs(inputs: Vec<Numbered<InputDecl>>) -> Interface {
    Interface::new(Ports {
        inputs,
        outputs: vec![],
    })
}

/// An instance resolves to its compound's interface as it is now, and to nothing where the set
/// holds no compound under its id.
#[test]
fn an_instance_resolves_to_its_interface_as_it_is() {
    let library = library();
    let mut set = GraphSet::default();
    set.insert_compound(s(1), Compound::new("one", inputs(vec![int_port(7)])));
    place(&mut set, GraphId::Main, n(1), s(1));
    let name = |set: &GraphSet| {
        set.scope(&library, GraphId::Main)
            .resolve(set.main().find(n(1)).unwrap())
            .map(|decl| decl.inputs()[0].name.clone())
    };
    assert_eq!(name(&set), Ok("x".to_owned()));

    let renamed = inputs(vec![Numbered::new(
        PortId::new(7),
        InputDecl::required("renamed", DataType::Int),
    )]);
    let change = set.plan_interface(s(1), renamed, &library, RefusedFate::Drop);
    set.write_interface(&change, ChangeSide::After);
    assert_eq!(name(&set), Ok("renamed".to_owned()));

    let mut lacking = GraphSet::from(set.main().clone());
    lacking.main_mut().find_mut(n(1)).unwrap().name = Some("orphan".to_owned());
    assert_eq!(
        lacking
            .scope(&library, GraphId::Main)
            .resolve(lacking.main().find(n(1)).unwrap())
            .map(|decl| decl.name().to_owned()),
        Err(Unresolved::MissingCompound)
    );
}

/// The `Inputs` node shows each interface input as an output of its type, and the `Outputs` node
/// each interface output as an optional input. A port made from an `Any` input is an `Any`
/// output, which feeds an `Any` input and no `Int` one. The `Inputs` node has no trigger, and the
/// `Outputs` node one per interface event alone.
#[test]
fn boundary_nodes_mirror_the_interface_under_the_one_admission_rule() {
    let library = library();
    let interface = Interface::new(Ports {
        inputs: vec![Numbered::new(
            PortId::new(1),
            InputDecl::required("anything", DataType::Any),
        )],
        outputs: vec![Numbered::new(
            PortId::new(2),
            OutputDecl::new("count", DataType::Int),
        )],
    });
    let mut compound = Compound::new("mirror", interface);
    let inputs = compound.graph.inputs_node();
    let outputs = compound.graph.outputs_node();
    let graph = &mut compound.graph;
    graph.insert(n(1), node(&library, ANY_SINK));
    graph.insert(n(2), node(&library, SINK));
    graph.insert(n(3), node(&library, TICKER));
    let mut set = GraphSet::default();
    set.insert_compound(s(1), compound);
    let scope = set.scope(&library, GraphId::Compound(s(1)));
    let graph = set.graph(GraphId::Compound(s(1)));

    let inputs_decl = scope.decl(graph.find(inputs).unwrap());
    assert!(inputs_decl.inputs().is_empty());
    assert_eq!(inputs_decl.outputs()[0].data_type, DataType::Any);
    let outputs_decl = scope.decl(graph.find(outputs).unwrap());
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

    let any_port = PortId::new(1);
    let from_inputs = Binding::Wire(OutputPort::new(inputs, any_port));
    assert_eq!(
        graph.admit(scope, InputPort::new(n(1), PortId::new(1)), &from_inputs),
        Ok(())
    );
    assert_eq!(
        graph.admit(scope, InputPort::new(n(2), PortId::new(1)), &from_inputs),
        Err(BindingRejection::WireMismatch)
    );
    assert_eq!(
        graph.admit(scope, InputPort::new(outputs, any_port), &from_inputs),
        Err(BindingRejection::UndeclaredInput)
    );

    assert_eq!(
        graph.admit_subscription(
            scope,
            Subscription::new(EventPort::new(n(3), PortId::new(1)), inputs)
        ),
        Err(SubscriptionRejection::ImpossibleTrigger)
    );
    assert_eq!(
        graph.admit_subscription(
            scope,
            Subscription::new(EventPort::new(n(3), PortId::new(1)), outputs)
        ),
        Err(SubscriptionRejection::UndeclaredTrigger),
        "an `Outputs` node has a trigger per interface event alone, and this interface has none"
    );
    assert_eq!(
        graph.admit_subscription(
            scope,
            Subscription::new(EventPort::new(n(3), PortId::new(1)), n(1))
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
    let interface = inputs(vec![Numbered::new(
        PortId::new(1),
        InputDecl::optional("gain", DataType::Float).default(2.0),
    )]);
    let compound = Compound::new("gain", interface);
    let mut graph = Graph::default();
    let node_id = graph.add_instance(s(1), &compound);
    assert_eq!(
        graph.wiring().bindings_on(node_id).collect::<Vec<_>>(),
        [(
            InputPort::new(node_id, PortId::new(1)),
            &Binding::Const(2.0.into())
        )]
    );
    let node = graph.find(node_id).unwrap();
    assert_eq!(node.compound(), Some(s(1)));
    assert_eq!(
        node.name, None,
        "an instance has no name of its own: it reads its compound's"
    );
    assert_eq!(
        node.cache(),
        Some(CacheMode::AUTO),
        "a new instance keeps what leaves it as any node does, while memory allows"
    );
}

/// A set round-trips with what its interfaces derive built again, and a file that names one
/// compound id twice is refused rather than loaded with the last.
#[test]
fn a_set_round_trips_and_a_repeated_compound_id_is_refused() {
    let mut set = GraphSet::default();
    set.insert_compound(s(1), Compound::new("one", inputs(vec![int_port(7)])));
    for format in SerdeFormat::ALL {
        let bytes = format.serialize(&set).unwrap();
        let back: GraphSet = format.deserialize(&bytes).unwrap();
        assert_eq!(back, set, "{format:?}");
    }
    // A file writes a compound as one value, its declaration's fields before its graph.
    let ron = String::from_utf8(SerdeFormat::Ron.serialize(&set).unwrap()).unwrap();
    let field = |name: &str| ron.find(&format!("{name}:")).unwrap();
    assert!(field("main") < field("compounds"));
    assert!(field("compounds") < field("name"));
    assert!(field("name") < field("description"));
    assert!(field("description") < field("interface"));
    assert!(ron.rfind("graph:").unwrap() > field("interface"));
    let back: GraphSet = SerdeFormat::Ron.deserialize(ron.as_bytes()).unwrap();
    let interface = |set: &GraphSet| set.compound(s(1)).unwrap().decl.interface.clone();
    assert_eq!(
        interface(&back).boundary_ports(Boundary::Inputs),
        interface(&set).boundary_ports(Boundary::Inputs)
    );

    let one = BTreeMap::from([(s(1), set.compound(s(1)).unwrap())]);
    let text = String::from_utf8(SerdeFormat::Ron.serialize(&one).unwrap()).unwrap();
    let entry = text
        .trim()
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap()
        .trim()
        .trim_end_matches(',');
    let main = String::from_utf8(SerdeFormat::Ron.serialize(&Graph::default()).unwrap()).unwrap();
    let repeated = format!("(main: {main}, compounds: {{{entry},{entry}}})");
    let error = SerdeFormat::Ron
        .deserialize::<GraphSet>(repeated.as_bytes())
        .unwrap_err();
    let chain = ErrorChain::new(&error).to_string();
    assert!(chain.contains("duplicate compound id"), "{chain}");
}

/// A const-only input takes a wire from the `Inputs` node's mirror of a const-only interface
/// input, which an instance binds to a constant alone, and from nothing else: not from a plain
/// interface input, not from a func's output.
#[test]
fn a_const_only_input_takes_a_wire_from_a_const_only_interface_input_alone() {
    let library = library();
    let interface = inputs(vec![
        Numbered::new(
            PortId::new(1),
            InputDecl::required("config", DataType::Int).const_only(),
        ),
        int_port(2),
    ]);
    let mut compound = Compound::new("configured", interface);
    let inputs_node = compound.graph.inputs_node();
    compound.graph.insert(n(1), node(&library, CONFIG_SINK));
    compound.graph.insert(n(2), node(&library, SRC));
    let mut set = GraphSet::default();
    set.insert_compound(s(1), compound);
    let scope = set.scope(&library, GraphId::Compound(s(1)));
    let graph = set.graph(GraphId::Compound(s(1)));
    let into_config = |producer: NodeId, port: u32| {
        graph.admit(
            scope,
            InputPort::new(n(1), PortId::new(1)),
            &Binding::Wire(OutputPort::new(producer, PortId::new(port))),
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
/// is not an instance; a graph's instances are every path that ends in it; a compound's uses are every node that places it, a ghost node's included, in
/// each graph; a compound no one uses is removed, and one in use refuses.
#[test]
fn paths_and_uses_read_the_instances_of_every_graph() {
    let library = library();
    let mut set = GraphSet::default();
    set.insert_compound(s(1), empty("inner"));
    set.insert_compound(s(2), empty("outer"));
    set.insert_compound(s(3), empty("unused"));
    place(&mut set, GraphId::Compound(s(2)), n(20), s(1));
    place(&mut set, GraphId::Main, n(2), s(2));
    place(&mut set, GraphId::Main, n(3), s(1));
    set.main_mut().insert(n(4), node(&library, SRC));
    set.main_mut().detach_into_ghosts(
        vec![Detaching {
            node_id: n(3),
            reason: DetachReason::TooLarge,
            saved: None,
        }],
        &mut Vec::new(),
    );

    let path = |ids: &[u128]| InstancePath::new(&ids.iter().copied().map(n).collect::<Vec<_>>());
    assert_eq!(set.graph_at(InstancePath::MAIN), Some(GraphId::Main));
    assert_eq!(set.graph_at(path(&[2])), Some(GraphId::Compound(s(2))));
    assert_eq!(set.graph_at(path(&[2, 20])), Some(GraphId::Compound(s(1))));
    assert_eq!(set.graph_at(path(&[4])), None, "a `src` places nothing");
    assert_eq!(
        set.graph_at(path(&[3])),
        None,
        "a ghost node places nothing"
    );
    assert_eq!(set.graph_at(path(&[9])), None);

    assert_eq!(set.instances(GraphId::Main), [InstancePath::MAIN]);
    // `s(1)` is placed in the main graph by the ghost node `n(3)` alone, which expands nothing.
    assert_eq!(set.instances(GraphId::Compound(s(1))), [path(&[2, 20])]);
    assert_eq!(set.instances(GraphId::Compound(s(2))), [path(&[2])]);
    assert_eq!(set.instances(GraphId::Compound(s(3))), []);
    place(&mut set, GraphId::Main, n(1), s(2));
    assert_eq!(
        set.instances(GraphId::Compound(s(1))),
        [path(&[1, 20]), path(&[2, 20])],
        "each instance of the graph that places it, in path order"
    );
    set.main_mut().detach_node(n(1));

    assert_eq!(
        set.uses(s(1)).collect::<Vec<_>>(),
        [
            NodeAddr::new(GraphId::Main, n(3)),
            NodeAddr::new(GraphId::Compound(s(2)), n(20)),
        ]
    );
    assert_eq!(
        set.uses(s(2)).collect::<Vec<_>>(),
        [NodeAddr::new(GraphId::Main, n(2))]
    );
    assert_eq!(set.uses(s(3)).count(), 0);

    assert!(set.remove_compound(s(3)).is_some());
    assert!(set.compound(s(3)).is_none());
    let in_use = panic::catch_unwind(AssertUnwindSafe(|| set.remove_compound(s(1))));
    assert!(in_use.is_err(), "a placed compound is not removed");
}

/// Each type an interface names that the library does not register is reported once, with every
/// port that names it, in compound order, inputs before outputs; a registered type is not.
#[test]
fn unknown_types_name_each_interface_port_that_uses_them() {
    let library = library();
    let unknown = DataType::Custom(CustomTypeId::from_u128(77));
    let port =
        |id: u32, data_type| Numbered::new(PortId::new(id), InputDecl::required("x", data_type));
    let mut set = GraphSet::default();
    set.insert_compound(
        s(1),
        Compound::new(
            "one",
            Interface::new(Ports {
                inputs: vec![port(1, DataType::Int), port(2, unknown)],
                outputs: vec![Numbered::new(PortId::new(3), OutputDecl::new("y", unknown))],
            }),
        ),
    );
    set.insert_compound(s(2), Compound::new("two", inputs(vec![port(4, unknown)])));
    let at = |compound, list, id: u32| InterfacePort {
        compound,
        list,
        port: PortId::new(id),
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

/// A save names each registered type the set's ports name, the library's or one a file gave a
/// type the library lacks, and no scalar; a load where the library lacks a type reads its name
/// from the manifest, and the next save writes it again; a type nothing named stays "unknown
/// type", and a registered type reads the library's name over a file's.
#[test]
fn a_file_names_the_types_its_ports_name_and_a_load_without_them_keeps_the_names() {
    let star = CustomTypeId::from_u128(77);
    let mode = EnumId::from_u128(78);
    let nameless = CustomTypeId::from_u128(79);
    let mut with = library();
    with.add_custom_type(star, CustomType::new("Star List"));
    with.add_enum(mode, EnumType::new("Mode", ["a"]));
    let port =
        |id: u32, data_type| Numbered::new(PortId::new(id), InputDecl::required("x", data_type));
    let mut set = GraphSet::default();
    set.insert_compound(
        s(1),
        Compound::new(
            "one",
            Interface::new(Ports {
                inputs: vec![port(1, DataType::Int), port(2, DataType::Custom(star))],
                outputs: vec![Numbered::new(
                    PortId::new(3),
                    OutputDecl::new("y", DataType::Enum(mode)),
                )],
            }),
        ),
    );
    let names = |manifest: &Manifest| {
        manifest
            .types()
            .iter()
            .map(|(type_ref, name)| (type_ref, name.to_owned()))
            .collect::<Vec<_>>()
    };
    let saved = set.manifest(&with);
    let expected = vec![
        (TypeRef::Custom(star), "Star List".to_owned()),
        (TypeRef::Enum(mode), "Mode".to_owned()),
    ];
    assert_eq!(names(&saved), expected, "no scalar, each type once");

    let without = library();
    let mut loaded = set.clone();
    loaded.reconcile(&without, &saved);
    assert_eq!(
        loaded.type_name(&without, DataType::Custom(star)),
        "Star List"
    );
    assert_eq!(
        loaded.type_name(&without, DataType::Custom(nameless)),
        "Unknown Type"
    );
    assert_eq!(loaded.type_name(&without, DataType::Int), "Integer");
    assert_eq!(
        names(&loaded.manifest(&without)),
        expected,
        "a save without the types writes the names it learned"
    );
    let mut renamed = library();
    renamed.add_custom_type(star, CustomType::new("Stars"));
    assert_eq!(
        loaded.type_name(&renamed, DataType::Custom(star)),
        "Stars",
        "the library's name wins"
    );
}

const A: PortId = PortId::new(101);
const B: PortId = PortId::new(102);
const SUM: PortId = PortId::new(201);

/// `adder`'s interface: `a` and `b` of `a_type` and `b_type`, and an `Int` `sum`.
fn adder_interface(a_type: DataType, b_type: DataType) -> Interface {
    Interface::new(Ports {
        inputs: vec![
            Numbered::new(A, InputDecl::required("a", a_type)),
            Numbered::new(B, InputDecl::required("b", b_type)),
        ],
        outputs: vec![Numbered::new(SUM, OutputDecl::new("sum", DataType::Int))],
    })
}

/// Compound 1, `adder`: its `Inputs` node's `a` and `b` into `add` 13, whose sum is the `Outputs`
/// node's. The main graph: `src` 1 into instance 2's `a`, a constant 7 on its `b`, and `sink` 3
/// reading its sum. So each interface port is bound in two graphs.
fn adder_set() -> (GraphSet, Library) {
    let library = library();
    let mut adder = Compound::new("adder", adder_interface(DataType::Int, DataType::Int));
    let (inputs, outputs) = (adder.graph.inputs_node(), adder.graph.outputs_node());
    adder.graph.insert(n(13), node(&library, ADD));
    let inner = &mut adder.graph;
    inner.set_input_binding(
        InputPort::new(n(13), PortId::new(1)),
        Binding::wire(inputs, A),
    );
    inner.set_input_binding(
        InputPort::new(n(13), PortId::new(2)),
        Binding::wire(inputs, B),
    );
    inner.set_input_binding(
        InputPort::new(outputs, SUM),
        Binding::wire(n(13), PortId::new(1)),
    );
    let mut set = GraphSet::default();
    set.insert_compound(s(1), adder);
    set.main_mut().insert(n(1), node(&library, SRC));
    place(&mut set, GraphId::Main, n(2), s(1));
    set.main_mut().insert(n(3), node(&library, SINK));
    let main = set.main_mut();
    main.set_input_binding(InputPort::new(n(2), A), Binding::wire(n(1), PortId::new(1)));
    main.set_input_binding(InputPort::new(n(2), B), Binding::Const(7i64.into()));
    main.set_input_binding(
        InputPort::new(n(3), PortId::new(1)),
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
            Numbered::new(
                a.id,
                InputDecl {
                    name: "alpha".to_owned(),
                    ..a.port
                },
            ),
        ],
        outputs: adder_interface(DataType::Int, DataType::Int)
            .outputs()
            .to_vec(),
    });
    let change = set.plan_interface(s(1), reordered.clone(), &library, RefusedFate::Drop);
    assert!(change.refused_bindings().is_empty());
    set.write_interface(&change, ChangeSide::After);
    assert_eq!(set.compound(s(1)).unwrap().decl.interface, reordered);
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
    let adder = set.compound(s(1)).unwrap();
    let (inputs, outputs) = (adder.graph.inputs_node(), adder.graph.outputs_node());
    let change = set.plan_interface(s(1), without.clone(), &library, RefusedFate::Drop);
    let inside = GraphId::Compound(s(1));
    let mut expected = vec![
        (
            GraphId::Main,
            InputPort::new(n(2), A),
            Binding::wire(n(1), PortId::new(1)),
        ),
        (
            GraphId::Main,
            InputPort::new(n(3), PortId::new(1)),
            Binding::wire(n(2), SUM),
        ),
        (
            inside,
            InputPort::new(n(13), PortId::new(1)),
            Binding::wire(inputs, A),
        ),
        (
            inside,
            InputPort::new(outputs, SUM),
            Binding::wire(n(13), PortId::new(1)),
        ),
    ];
    // Graph by graph, each in the order the graph keeps its bindings.
    expected.sort_by_key(|(graph, port, _)| (*graph, *port));
    let refused = |change: &InterfaceChange| {
        change
            .refused_bindings()
            .iter()
            .map(|refused| (refused.graph, refused.port, refused.ghost.binding()))
            .collect::<Vec<_>>()
    };
    assert_eq!(refused(&change), expected);
    assert_eq!(
        change.before(),
        &before.compound(s(1)).unwrap().decl.interface
    );
    assert_eq!(change.after(), &without);

    set.write_interface(&change, ChangeSide::After);
    assert!(
        set.main()
            .wiring()
            .binding(InputPort::new(n(2), A))
            .is_none()
    );
    assert!(
        set.main().ghost_wiring().is_empty(),
        "a person's edit drops"
    );
    set.validate_with(&library).unwrap();
    set.write_interface(&change, ChangeSide::Before);
    assert_eq!(set, before);

    // As an update does it: the same bindings kept as ghosts, each lost port under the name and
    // type the old interface gave it, and undo makes them live again.
    let kept = set.plan_interface(s(1), without, &library, RefusedFate::Ghost);
    assert_eq!(refused(&kept), expected);
    set.write_interface(&kept, ChangeSide::After);
    let a_port = |name: &str| {
        Some(SavedPort {
            name: name.to_owned(),
            data_type: DataType::Int,
        })
    };
    let on_instance = set
        .main()
        .ghost_wiring()
        .binding(InputPort::new(n(2), A))
        .unwrap();
    assert_eq!(on_instance.input, a_port("a"));
    let from_instance = set
        .main()
        .ghost_wiring()
        .binding(InputPort::new(n(3), PortId::new(1)))
        .unwrap();
    assert_eq!(
        from_instance.source,
        GhostSource::Wire {
            output: OutputPort::new(n(2), SUM),
            saved: a_port("sum"),
        }
    );
    set.validate_with(&library).unwrap();

    // A later update that brings the ports back refuses nothing and touches no ghost: each stays
    // a ghost, its saved name outranked by the declaration, until a load or a Restore makes it
    // live, and the set is one a compile takes.
    let back = set.plan_interface(
        s(1),
        before.compound(s(1)).unwrap().decl.interface.clone(),
        &library,
        RefusedFate::Ghost,
    );
    assert!(back.refused_bindings().is_empty());
    set.write_interface(&back, ChangeSide::After);
    set.validate_with(&library).unwrap();
    assert_eq!(set.main().ghost_wiring().bindings().len(), 2);
    set.write_interface(&back, ChangeSide::Before);
    set.write_interface(&kept, ChangeSide::Before);
    assert_eq!(set, before);
}

/// A port retyped drops only what the new type refuses: `b` as a `String` refuses the constant 7
/// on the instance and its wire into the `Int` input of `add`, and keeps every binding of `a`.
#[test]
fn an_interface_port_retyped_drops_only_what_the_new_type_refuses() {
    let (mut set, library) = adder_set();
    let inputs = set.compound(s(1)).unwrap().graph.inputs_node();
    let change = set.plan_interface(
        s(1),
        adder_interface(DataType::Int, DataType::String),
        &library,
        RefusedFate::Drop,
    );
    assert_eq!(
        change
            .refused_bindings()
            .iter()
            .map(|refused| (
                refused.graph,
                refused.port,
                refused.ghost.binding(),
                refused.ghost.input.clone()
            ))
            .collect::<Vec<_>>(),
        [
            (
                GraphId::Main,
                InputPort::new(n(2), B),
                Binding::Const(7i64.into()),
                None,
            ),
            (
                GraphId::Compound(s(1)),
                InputPort::new(n(13), PortId::new(2)),
                Binding::wire(inputs, B),
                None,
            ),
        ],
        "a retyped port is still declared, so no ghost names it"
    );
    set.write_interface(&change, ChangeSide::After);
    set.validate_with(&library).unwrap();
}

/// A node of a func the library lacks is kept as a ghost node, stays one at each load that still lacks
/// it, and is attached again by the first load whose library holds it.
#[test]
fn a_ghost_node_of_a_missing_func_stays_one_until_the_library_holds_it() {
    let mut library = library();
    let missing = FuncId::from_u128(99);
    let mut set = GraphSet::default();
    set.main_mut()
        .insert(n(1), internals::unresolved_node(missing));
    assert_eq!(
        ghosted(
            &set.reconcile(&library, &Manifest::default())
                .of(GraphId::Main)
        ),
        [(n(1), DetachReason::Unresolved(Unresolved::MissingFunc))]
    );
    assert!(
        set.reconcile(&library, &Manifest::default()).is_empty(),
        "a ghost node that still does not resolve stays"
    );

    library.add(internals::stub_func(missing, "found").build());
    assert_eq!(
        set.reconcile(&library, &Manifest::default())
            .of(GraphId::Main),
        [Repair::Reattached { node_id: n(1) }]
    );
    assert!(set.main().find(n(1)).is_some());
    assert_eq!(set.main().ghost_nodes().len(), 0);
}

/// A digest is of the definition alone: two sets holding one definition under two ids, its
/// nodes added in two orders and its placed compound under two ids, digest it alike; a renamed
/// inner node, a changed placed compound, or a ghost it keeps, changes it.
#[test]
fn a_digest_names_the_definition_and_not_its_ids() {
    let library = library();
    // A definition whose nodes, the boundary nodes among them, go in under fixed ids in `order`.
    let definition = |name: &str, order: &[(u128, Node)]| {
        let mut graph = Graph::default();
        for (id, held) in order {
            graph.insert(n(*id), held.clone());
        }
        Compound {
            decl: CompoundDecl {
                name: name.to_string(),
                description: None,
                interface: Interface::default(),
            },
            graph,
        }
    };
    let build = |outer: CompoundId, inner: CompoundId, reversed: bool, inner_name: &str| {
        let mut set = GraphSet::default();
        let placed = definition(
            inner_name,
            &[
                (1, Node::from(Boundary::Inputs)),
                (2, Node::from(Boundary::Outputs)),
                (5, node(&library, SRC)),
            ],
        );
        let instance = Node::instance(inner);
        set.insert_compound(inner, placed);
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
        set.insert_compound(outer, definition("outer", &order));
        set
    };
    let one = build(s(1), s(2), false, "inner");
    let two = build(s(7), s(8), true, "inner");
    assert_eq!(one.digest(s(1)), two.digest(s(7)));
    assert_eq!(one.digest(s(2)), two.digest(s(8)));

    let mut renamed = build(s(1), s(2), false, "inner");
    renamed
        .graph_mut(GraphId::Compound(s(1)))
        .find_mut(n(10))
        .unwrap()
        .name = Some("renamed".to_string());
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
        "a placed compound's change reaches the one that places it"
    );

    // What a graph keeps is part of its definition: a ghost node and a ghost on an input each
    // change the digest.
    let mut ghosted = build(s(1), s(2), false, "inner");
    ghosted
        .graph_mut(GraphId::Compound(s(1)))
        .detach_into_ghosts(
            vec![Detaching {
                node_id: n(11),
                reason: DetachReason::TooLarge,
                saved: None,
            }],
            &mut Vec::new(),
        );
    assert_ne!(ghosted.digest(s(1)), one.digest(s(1)));
    let mut kept = build(s(1), s(2), false, "inner");
    kept.graph_mut(GraphId::Compound(s(1))).set_input_state(
        InputPort::new(n(11), PortId::new(1)),
        InputState::Ghost(GhostBinding::from(Binding::wire(n(10), PortId::new(1)))),
    );
    assert_ne!(kept.digest(s(1)), one.digest(s(1)));
    assert_ne!(kept.digest(s(1)), ghosted.digest(s(1)));
}

/// A subscription lands on a sink's own trigger, on an instance's when its interface declares a
/// trigger, and on no other trigger of the instance, which no node of its kind has and a load
/// drops even as a ghost, nor on a func that is no sink, which a load keeps as a ghost; an
/// interface edit that takes an event away refuses the subscriptions from it outside and to it
/// inside, which a person's edit drops and an update keeps as ghosts under the event's name, and
/// undo puts them back live. One that takes the trigger away keeps the subscription to it as a
/// ghost, which a load under an interface with a trigger again brings back.
#[test]
fn a_subscription_lands_on_a_declared_trigger_and_an_interface_edit_refuses_it() {
    use crate::decl::ports::EventDecl;
    use crate::decl::saved_decl::SavedEvent;
    use crate::graph::error::SubscriptionRejection;
    use crate::graph::ghost_wiring::{GhostSubscription, SubscriptionState};
    use crate::graph::interface::EventPorts;
    use crate::graph::interface_change::{ChangeSide, RefusedFate};
    use crate::graph::subscription::Subscription;
    use crate::identity::{EventPort, TriggerPort};
    use crate::internals::test_graph::sets::TICKER;

    const OUT: PortId = PortId::new(301);
    let library = library();
    let interface = Interface::default()
        .with_event_ports(EventPorts {
            events: vec![Numbered::new(OUT, EventDecl::new("out"))],
            trigger: Some(EventDecl::new("in")),
        })
        .unwrap();
    let mut relay = Compound::new("relay", interface.clone());
    relay.graph.insert(n(5), node(&library, TICKER));
    let passed = Subscription::new(
        EventPort::new(n(5), PortId::new(1)),
        TriggerPort::new(relay.graph.outputs_node(), OUT),
    );
    relay.graph.subscribe(passed.event, passed.trigger);
    let mut set = GraphSet::default();
    set.insert_compound(s(1), relay);
    set.main_mut().insert(n(1), node(&library, TICKER));
    set.main_mut().insert(n(2), Node::instance(s(1)));
    set.main_mut().insert(n(3), node(&library, SINK));
    set.main_mut().insert(n(4), node(&library, TICKER));
    let tick = EventPort::new(n(1), PortId::new(1));
    let admit = |set: &GraphSet, trigger: TriggerPort| {
        set.main().admit_subscription(
            set.scope(&library, GraphId::Main),
            Subscription::new(tick, trigger),
        )
    };
    assert_eq!(admit(&set, TriggerPort::from(n(2))), Ok(()));
    assert_eq!(
        admit(&set, TriggerPort::new(n(2), PortId::new(2))),
        Err(SubscriptionRejection::ImpossibleTrigger),
        "an instance's one trigger is its own"
    );
    assert_eq!(
        admit(&set, TriggerPort::from(n(3))),
        Ok(()),
        "a sink is triggered"
    );
    assert_eq!(
        admit(&set, TriggerPort::from(n(4))),
        Err(SubscriptionRejection::UndeclaredTrigger),
        "a func that is no sink has no trigger"
    );
    // A load keeps a subscription on a trigger its func no longer has as a ghost: a library may
    // make the func a sink again.
    let lost = Subscription::new(tick, n(4));
    set.main_mut().subscribe(lost.event, n(4));
    assert!(!set.reconcile(&library, &Manifest::default()).is_empty());
    assert_eq!(
        set.main().subscription_state(lost),
        SubscriptionState::Ghost(GhostSubscription::default())
    );
    set.main_mut().unsubscribe(lost.event, n(4));
    // A file's ghost on a trigger no node of its kind has is dropped and reported.
    let impossible = Subscription::new(tick, TriggerPort::new(n(3), PortId::new(9)));
    set.main_mut().set_subscription_state(
        impossible,
        SubscriptionState::Ghost(GhostSubscription::default()),
    );
    assert!(!set.reconcile(&library, &Manifest::default()).is_empty());
    assert_eq!(
        set.main().subscription_state(impossible),
        SubscriptionState::Absent
    );

    let triggered = Subscription::new(tick, n(2));
    set.main_mut().subscribe(triggered.event, n(2));
    let untriggered = interface
        .clone()
        .with_event_ports(EventPorts {
            events: interface.events().to_vec(),
            trigger: None,
        })
        .unwrap();
    let kept = set.plan_interface(s(1), untriggered, &library, RefusedFate::Ghost);
    assert_eq!(kept.refused_subscriptions().len(), 1);
    set.write_interface(&kept, ChangeSide::After);
    assert_eq!(
        set.main().subscription_state(triggered),
        SubscriptionState::Ghost(GhostSubscription::default())
    );
    assert_eq!(
        admit(&set, TriggerPort::from(n(2))),
        Err(SubscriptionRejection::UndeclaredTrigger)
    );
    set.validate_with(&library).unwrap();
    // A load under an interface that has a trigger again brings the ghost back live.
    let regained = set.plan_interface(s(1), interface, &library, RefusedFate::Ghost);
    assert!(regained.refused_subscriptions().is_empty());
    set.write_interface(&regained, ChangeSide::After);
    assert!(!set.reconcile(&library, &Manifest::default()).is_empty());
    assert_eq!(
        set.main().subscription_state(triggered),
        SubscriptionState::Live
    );
    set.main_mut().unsubscribe(triggered.event, n(2));

    let out = Subscription::new(EventPort::new(n(2), OUT), n(3));
    set.main_mut().subscribe(out.event, n(3));
    set.validate_with(&library).unwrap();
    let before = set.clone();
    let without = Interface::default();
    let dropped = set.plan_interface(s(1), without.clone(), &library, RefusedFate::Drop);
    assert_eq!(
        dropped.refused_subscriptions().len(),
        2,
        "the event outside and its trigger inside"
    );
    set.write_interface(&dropped, ChangeSide::After);
    assert_eq!(
        set.main().subscription_state(out),
        SubscriptionState::Absent
    );
    set.validate_with(&library).unwrap();
    set.write_interface(&dropped, ChangeSide::Before);
    assert_eq!(set, before);

    let kept = set.plan_interface(s(1), without, &library, RefusedFate::Ghost);
    set.write_interface(&kept, ChangeSide::After);
    assert_eq!(
        set.main().subscription_state(out),
        SubscriptionState::Ghost(GhostSubscription {
            event: Some(SavedEvent {
                name: "out".to_owned(),
            }),
            trigger: None,
        })
    );
    assert_eq!(
        set.graph(GraphId::Compound(s(1)))
            .subscription_state(passed),
        SubscriptionState::Ghost(GhostSubscription {
            event: None,
            trigger: Some(SavedEvent {
                name: "out".to_owned(),
            }),
        }),
        "the ticker inside still fires; the `Outputs` node lost the trigger"
    );
    set.validate_with(&library).unwrap();
    set.write_interface(&kept, ChangeSide::Before);
    assert_eq!(set, before);

    // A ghost on a trigger the interface shows again, of an event its emitter does not declare,
    // stays a ghost: a load takes the trigger's saved name off it, as the declaration names the
    // trigger now, and leaves the event's.
    let stray = Subscription::new(
        EventPort::new(passed.event.node_id, PortId::new(9)),
        passed.trigger,
    );
    let saved = |name: &str| {
        Some(SavedEvent {
            name: name.to_owned(),
        })
    };
    set.graph_mut(GraphId::Compound(s(1)))
        .set_subscription_state(
            stray,
            SubscriptionState::Ghost(GhostSubscription {
                event: saved("gone"),
                trigger: saved("out"),
            }),
        );
    // Naming a ghost's end is no repair a load reports.
    assert!(set.reconcile(&library, &Manifest::default()).is_empty());
    assert_eq!(
        set.graph(GraphId::Compound(s(1))).subscription_state(stray),
        SubscriptionState::Ghost(GhostSubscription {
            event: saved("gone"),
            trigger: None,
        })
    );
}

/// An interface edit that removes a port an existing ghost names saves the port's name and type on
/// the ghost, at each end: outside, a ghost binding's source on the instance and a ghost
/// subscription's event; inside, a ghost binding's input and a ghost subscription's trigger on the
/// `Outputs` node. An undo takes them off again, and a ghost the edit leaves alone keeps what it
/// had.
#[test]
fn an_interface_edit_names_the_ports_it_removes_on_existing_ghosts() {
    use crate::data::const_value::ConstValue;
    use crate::decl::ports::{EventDecl, OutputDecl};
    use crate::decl::saved_decl::{SavedEvent, SavedPort};
    use crate::graph::ghost_wiring::{
        GhostBinding, GhostSource, GhostSubscription, InputState, SubscriptionState,
    };
    use crate::graph::interface::EventPorts;
    use crate::graph::interface_change::{ChangeSide, RefusedFate};
    use crate::graph::subscription::Subscription;
    use crate::identity::{EventPort, OutputPort, TriggerPort};
    use crate::internals::test_graph::sets::TICKER;

    const RESULT: PortId = PortId::new(201);
    const OUT: PortId = PortId::new(301);
    let library = library();
    let interface = Interface::new(Ports {
        inputs: vec![],
        outputs: vec![Numbered::new(
            RESULT,
            OutputDecl::new("result", DataType::Int),
        )],
    })
    .with_event_ports(EventPorts {
        events: vec![Numbered::new(OUT, EventDecl::new("out"))],
        trigger: None,
    })
    .unwrap();
    let mut relay = Compound::new("relay", interface);
    let outputs = relay.graph.outputs_node();
    relay.graph.insert(n(5), node(&library, TICKER));
    // Inside, the ticker declares no output 9 and no event 7: each ghost's far end is unknown.
    let fed = InputPort::new(outputs, RESULT);
    relay.graph.set_input_state(
        fed,
        InputState::Ghost(GhostBinding {
            input: None,
            source: GhostSource::Wire {
                output: OutputPort::new(n(5), PortId::new(9)),
                saved: None,
            },
        }),
    );
    let passed = Subscription::new(
        EventPort::new(n(5), PortId::new(7)),
        TriggerPort::new(outputs, OUT),
    );
    relay.graph.set_subscription_state(
        passed,
        SubscriptionState::Ghost(GhostSubscription::default()),
    );
    let mut set = GraphSet::default();
    set.insert_compound(s(1), relay);
    set.main_mut().insert(n(2), Node::instance(s(1)));
    set.main_mut().insert(n(3), node(&library, SINK));
    set.main_mut().insert(n(4), node(&library, TICKER));
    // The sink declares no input 9, and the ticker no trigger: each ghost's other end is live.
    let wired = InputPort::new(n(3), PortId::new(9));
    let wire = GhostBinding {
        input: None,
        source: GhostSource::Wire {
            output: OutputPort::new(n(2), RESULT),
            saved: None,
        },
    };
    let constant = InputPort::new(n(3), PortId::new(8));
    let kept = GhostBinding {
        input: None,
        source: GhostSource::Const(ConstValue::Int(4)),
    };
    let heard = Subscription::new(EventPort::new(n(2), OUT), TriggerPort::from(n(4)));
    let main = set.main_mut();
    main.set_input_state(wired, InputState::Ghost(wire));
    main.set_input_state(constant, InputState::Ghost(kept.clone()));
    main.set_subscription_state(
        heard,
        SubscriptionState::Ghost(GhostSubscription::default()),
    );
    let before = set.clone();

    let change = set.plan_interface(s(1), Interface::default(), &library, RefusedFate::Drop);
    assert!(
        change.refused_bindings().is_empty(),
        "no live wiring touches the instance"
    );
    set.write_interface(&change, ChangeSide::After);
    assert_eq!(
        set.main().input_state(wired),
        InputState::Ghost(GhostBinding {
            input: None,
            source: GhostSource::Wire {
                output: OutputPort::new(n(2), RESULT),
                saved: Some(SavedPort {
                    name: "result".to_owned(),
                    data_type: DataType::Int,
                }),
            },
        })
    );
    assert_eq!(set.main().input_state(constant), InputState::Ghost(kept));
    assert_eq!(
        set.main().subscription_state(heard),
        SubscriptionState::Ghost(GhostSubscription {
            event: Some(SavedEvent {
                name: "out".to_owned(),
            }),
            trigger: None,
        })
    );
    let inside = set.graph(GraphId::Compound(s(1)));
    assert_eq!(
        inside.input_state(fed),
        InputState::Ghost(GhostBinding {
            input: Some(SavedPort {
                name: "result".to_owned(),
                data_type: DataType::Int,
            }),
            source: GhostSource::Wire {
                output: OutputPort::new(n(5), PortId::new(9)),
                saved: None,
            },
        })
    );
    assert_eq!(
        inside.subscription_state(passed),
        SubscriptionState::Ghost(GhostSubscription {
            event: None,
            trigger: Some(SavedEvent {
                name: "out".to_owned(),
            }),
        })
    );
    set.write_interface(&change, ChangeSide::Before);
    assert_eq!(set, before);
}
