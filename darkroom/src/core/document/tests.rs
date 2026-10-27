use std::sync::Arc;

use common::SerdeFormat;
use palantir::DockOperation;

use scenarium::{EventPort, InputPort, OutputPort};

use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::core::document::tie::Tie;
use crate::core::document::wire_key::{DataWire, WireKey};

/// Every `NodeId`-keyed cache sweeps against one rule, and the two
/// narrowings of it are strict subsets — so a node that is gone is gone by
/// all of them at once, and nothing can be retained by one sweep while
/// another has released it.
///
/// Re-diverging the predicates is what this catches.
#[test]
fn node_liveness_is_one_rule_with_two_declared_narrowings() {
    let mut fixture = DocumentFixture::default();
    let preview_func = preview_sink::func(Arc::default());
    fixture.library.add(preview_func.clone());
    let preview = fixture.document.graph.add(Node::from(&preview_func));
    let plain = fixture.stub_at(Vec2::ZERO);
    let mut document = fixture.document;

    // The shared rule answers for a graph node and a stub alike.
    for node in [preview, plain] {
        assert!(document.holds_node(node), "the document holds {node:?}");
    }
    assert!(document.holds_preview_node(preview));
    assert!(
        !document.holds_preview_node(plain),
        "a stub retains no published value"
    );
    assert!(
        !document.holds_tab(TabRef::ImageViewer(plain)),
        "a stub has no viewer"
    );

    // The tab lift is the same rule asked of a tab: a viewer dies exactly
    // with its node, and the two non-node tabs never do.
    assert!(document.holds_tab(TabRef::ImageViewer(preview)));
    assert!(document.holds_tab(TabRef::Graph));
    assert!(document.holds_tab(TabRef::Preferences));

    document.graph.detach_node(preview);
    assert!(!document.holds_node(preview), "the rule says gone");
    assert!(
        !document.holds_preview_node(preview),
        "and so does the narrowing"
    );
    assert!(
        !document.holds_tab(TabRef::ImageViewer(preview)),
        "and so does the tab lift"
    );
    assert!(
        document.holds_node(plain),
        "the surviving node is untouched"
    );
}

/// A removal closes the node's viewer tab: every removal, by an edit, an
/// undo or a redo, goes through `remove_node`, so no tab outlives its node.
#[test]
fn removing_a_node_closes_its_viewer() {
    let mut fixture = DocumentFixture::sample();
    let node_id = fixture.add(&preview_sink::func(Arc::default()));
    let mut document = fixture.document;
    let primary = document.layout.primary().id;
    document
        .layout
        .find_or_insert(TabRef::ImageViewer(node_id), primary);

    document.remove_node(node_id);
    assert!(
        !document
            .layout
            .all_tabs()
            .any(|tab| tab == TabRef::ImageViewer(node_id))
    );
    document.validate().expect("no tab names the removed node");
}

/// The load repairs the view and the layout around the graph, in order,
/// and reports each repair: a bad viewport resets, a placement for no node
/// goes, a non-finite position and a missing placement go to the origin, a
/// selection entry with no placement goes, and a viewer on a node that is
/// no preview node closes. A layout that pins another tab resets whole.
#[test]
fn reconcile_repairs_the_view_and_the_layout() {
    let mut fixture = DocumentFixture::default();
    let shown = fixture.node_at(Vec2::new(f32::NAN, 0.0));
    let unplaced = fixture.node_at(Vec2::ZERO);
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    let ghost = NodeId::unique();
    document.main_view.viewport = Viewport {
        pan: Vec2::ZERO,
        zoom: 0.0,
    };
    document.main_view.node_placements.remove(&unplaced);
    document
        .main_view
        .node_placements
        .insert(ghost, NodePlacement::default());
    document.main_view.selected = [unplaced].map(ItemId::Node).into_iter().collect();
    let primary = document.layout.primary().id;
    document
        .layout
        .find_or_insert(TabRef::ImageViewer(shown), primary);

    let report = document.reconcile(&library);
    assert_eq!(
        report.repairs,
        [
            DocumentRepair::ViewportReset,
            DocumentRepair::PlacementDropped { node_id: ghost },
            DocumentRepair::PosReset { node_id: shown },
            DocumentRepair::Placed { node_id: unplaced },
            DocumentRepair::ViewerClosed { node_id: shown },
        ]
    );
    assert_eq!(document.main_view.node_placements[&shown].pos, Vec2::ZERO);
    assert_eq!(
        document.main_view.node_placements[&unplaced].pos,
        Vec2::ZERO
    );
    assert!(
        document
            .main_view
            .selected
            .contains(&ItemId::Node(unplaced)),
        "placed before the selection is checked, so it stays selected"
    );
    document.validate().expect("a reconciled document is valid");
    assert!(
        document.reconcile(&library).is_empty(),
        "nothing is left to repair"
    );

    document.layout = DockState::new(DOCK_SEED, TabRef::Preferences);
    assert_eq!(
        document.reconcile(&library).repairs,
        [DocumentRepair::LayoutReset]
    );
    assert_eq!(document.layout, new_layout());
}

/// The load repairs the ties and the routes, in order, and reports each
/// repair. Routes go in wire order: on `w1 = a.0 → b.0`, through `[t1, ghost,
/// t1]`, the step to no tie and the repeat go; the route keyed by `a.0 → b.1`,
/// a wire the graph does not hold, goes; on `w3 = a.0 → b.2`, through
/// `[ghost]`, the step goes and the route left empty with it. Then ties in id
/// order: `t1` at NaN moves to the origin, `t2` pinned to 400° turns itself,
/// `t3`, which no route names, goes, and so does `t4`, which only the dropped
/// route named. Last, the selected `t3` leaves the selection.
#[test]
fn reconcile_repairs_the_ties() {
    let mut fixture = DocumentFixture::default();
    let [a, b] = [0.0, 220.0].map(|x| fixture.node_at(Vec2::new(x, 0.0)));
    let w1 = fixture.wire(OutputPort::new(a, 0), InputPort::new(b, 0));
    let w2 = fixture.subscribe(EventPort::new(a, 0), b);
    let w3 = fixture.wire(OutputPort::new(a, 0), InputPort::new(b, 2));
    let unheld = WireKey::Data(DataWire {
        output: OutputPort::new(a, 0),
        input: InputPort::new(b, 1),
    });
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    let [t1, t2, t3, t4, ghost] = [1, 2, 3, 4, 5].map(TieId::from_u128);
    let view = &mut document.main_view;
    for (tie, pos, angle) in [
        (t1, Vec2::new(f32::NAN, 0.0), TieAngle::Auto),
        (t2, Vec2::ZERO, TieAngle::Pinned(400.0)),
        (t3, Vec2::ZERO, TieAngle::Auto),
        (t4, Vec2::ZERO, TieAngle::Pinned(90.0)),
    ] {
        view.ties.insert(tie, Tie { pos, angle });
    }
    view.routes = [
        (w1, Route::unchecked([t1, ghost, t1])),
        (unheld, Route::unchecked([t4])),
        (w3, Route::unchecked([ghost])),
        (w2, Route::unchecked([t2])),
    ]
    .into_iter()
    .collect();
    view.selected = [ItemId::Tie(t1), ItemId::Tie(t3)].into_iter().collect();

    let report = document.reconcile(&library);
    assert_eq!(
        report.repairs,
        [
            DocumentRepair::RouteStepDropped {
                wire: w1,
                tie: ghost
            },
            DocumentRepair::RouteStepRepeated { wire: w1, tie: t1 },
            DocumentRepair::RouteDropped { wire: unheld },
            DocumentRepair::RouteStepDropped {
                wire: w3,
                tie: ghost
            },
            DocumentRepair::EmptyRouteDropped { wire: w3 },
            DocumentRepair::TiePosReset { tie: t1 },
            DocumentRepair::TieAngleFreed { tie: t2 },
            DocumentRepair::TieDropped { tie: t3 },
            DocumentRepair::TieDropped { tie: t4 },
            DocumentRepair::SelectionDropped {
                item: ItemId::Tie(t3)
            },
        ]
    );
    let view = &document.main_view;
    assert_eq!(
        view.routes,
        [(w1, Route::new([t1])), (w2, Route::new([t2]))]
            .into_iter()
            .collect()
    );
    assert_eq!(
        view.ties,
        [
            (
                t1,
                Tie {
                    pos: Vec2::ZERO,
                    angle: TieAngle::Auto
                }
            ),
            (
                t2,
                Tie {
                    pos: Vec2::ZERO,
                    angle: TieAngle::Auto
                }
            ),
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(view.selected, [ItemId::Tie(t1)].into_iter().collect());
    document.validate().expect("a reconciled document is valid");
    assert!(
        document.reconcile(&library).is_empty(),
        "nothing is left to repair"
    );
}

/// Ties, routes and a selected tie round-trip through RON, the format a
/// document is saved in.
#[test]
fn ties_round_trip_as_ron() {
    let mut fixture = DocumentFixture::default();
    let [a, b] = [0.0, 220.0].map(|x| fixture.node_at(Vec2::new(x, 0.0)));
    let data = fixture.wire(OutputPort::new(a, 0), InputPort::new(b, 0));
    let event = fixture.subscribe(EventPort::new(a, 0), b);
    let first = fixture.tie(Vec2::new(80.0, 10.0), [data, event]);
    let second = fixture.tie(Vec2::new(140.0, 10.0), [data]);
    let mut document = fixture.document;
    document.main_view.ties.get_mut(&second).unwrap().angle = TieAngle::Pinned(45.0);
    document.main_view.selected = [ItemId::Node(a), ItemId::Tie(first)].into_iter().collect();
    document.validate().unwrap();

    let bytes = SerdeFormat::Ron.serialize(&document).unwrap();
    let back: Document = SerdeFormat::Ron.deserialize(&bytes).unwrap();
    assert_eq!(back.main_view, document.main_view);
    assert_eq!(
        back.main_view.routes[&data],
        Route::new([first, second]),
        "a route keeps its order"
    );
}

#[test]
fn dock_layout_round_trips_as_ron() {
    use palantir::{DockDrop, SplitSide};

    let mut fixture = DocumentFixture::sample();
    let node_id = fixture.add(&preview_sink::func(Arc::default()));
    let mut document = fixture.document;
    let primary = document.layout.primary().id;
    document.layout.find_or_insert(TabRef::Preferences, primary);
    document
        .layout
        .find_or_insert(TabRef::ImageViewer(node_id), primary);
    // A split pane too, so the whole tree shape round-trips — not
    // just a flat strip.
    document.layout.apply(DockOperation::MoveTab {
        tab: TabRef::ImageViewer(node_id),
        to: DockDrop::Split {
            group: primary,
            side: SplitSide::Right,
        },
    });
    let bytes = SerdeFormat::Ron
        .serialize(&document)
        .expect("serialize with dock layout");
    let back: Document = SerdeFormat::Ron.deserialize(&bytes).expect("deserialize");
    back.validate().expect("round-tripped document is valid");
    assert_eq!(
        back.layout, document.layout,
        "the split tree (groups, focus, ratio) round-trips through RON"
    );
}

/// The sample validates, and a layout that pins another tab, or was
/// saved under another dock seed, does not — each would pass palantir's
/// own load check.
#[test]
fn validation_holds_the_layout_to_the_editors_dock() {
    let mut document = DocumentFixture::sample().document;
    document.validate().unwrap();

    document.layout = DockState::new(DOCK_SEED, TabRef::Preferences);
    assert!(matches!(
        document.validate(),
        Err(DocumentValidationError::PinnedTab {
            tab: TabRef::Preferences
        })
    ));

    document.layout = DockState::new("another.dock", TabRef::Graph);
    assert!(matches!(
        document.validate(),
        Err(DocumentValidationError::ForeignDock)
    ));
}

/// A path a binding holds is absolute or unset in memory: a stub's relative one fails the
/// check and names its node, while an absolute one and an empty one pass.
#[test]
fn validation_holds_paths_absolute() {
    use std::path::{self, Path};

    use scenarium::{BindingEntry, ConstValue, DetachedNodeParts, InputPort};

    let mut fixture = DocumentFixture::sample();
    let stub = fixture.stub_at(Vec2::ZERO);
    let mut document = fixture.document;
    let with_path = |document: &mut Document, value: &str| {
        let record = document.stubs.remove(stub).unwrap();
        document.stubs.insert(
            DetachedNode::new(DetachedNodeParts {
                node_id: stub,
                node: record.node().clone(),
                bindings: vec![BindingEntry {
                    port: InputPort::new(stub, 0),
                    binding: Binding::Const(ConstValue::FsPath(value.to_string())),
                }],
                subscriptions: Vec::new(),
            })
            .unwrap(),
        );
    };
    let absolute = path::absolute(Path::new("x.tiff")).unwrap();
    for valid in [absolute.to_str().unwrap(), ""] {
        with_path(&mut document, valid);
        document.validate().unwrap();
    }
    with_path(&mut document, "inputs/x.tiff");
    assert!(matches!(
        document.validate(),
        Err(DocumentValidationError::RelativePath { node_id, path })
            if node_id == stub && path == "inputs/x.tiff"
    ));
}
