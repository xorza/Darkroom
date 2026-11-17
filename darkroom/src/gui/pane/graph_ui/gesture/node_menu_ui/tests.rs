use std::collections::BTreeSet;

use glam::UVec2;
use palantir::WidgetId;
use scenarium::{Binding, GraphId, InputPort, NodeId, PortId, ProgramId};

use crate::core::document::internals::DocumentFixture;
use crate::core::document::item_id::ItemId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::RunCommand;
use crate::gui::pane::graph_ui::internals::CanvasHarness;

/// Room for three nodes in a row *and* the opened menu below them.
const SURFACE: UVec2 = UVec2::new(1600, 900);

/// Every row the menu offers over a runnable node, in layout order.
const RUN: usize = 0;
const DUPLICATE: usize = 1;
const DUPLICATE_WITH_INCOMING: usize = 2;
/// Replace with… sits between the duplicates and Remove.
const REMOVE: usize = 4;
const ROW_COUNT: usize = 5;

/// Open the menu on `on`, click its `row`th item, and return the intents the picking frame
/// raised. The open and the pick are separate frames because the menu has to record before an
/// item can be hit, as in the app, which lets the pick read back the selection the open committed.
fn pick(h: &mut CanvasHarness, on: NodeId, row: usize) -> Vec<GraphIntent> {
    let rows = open_menu(h, on);
    assert_eq!(
        rows.len(),
        ROW_COUNT,
        "the sweep found rows the menu does not offer, or missed some"
    );
    h.ui.click(rows[row]);
    h.frame()
}

/// Open the menu on `on` and return its rows.
fn open_menu(h: &mut CanvasHarness, on: NodeId) -> Vec<WidgetId> {
    // Two frames so every node body has recorded and carries a hit-testable
    // rect to aim the right-click at, then two more so the opened menu has.
    h.prime(2);
    let anchor = h.node_center(on);
    h.ui.right_click(anchor);
    h.prime(2);
    h.menu_rows(anchor)
}

/// A ghost node neither runs nor copies, so its menu offers Replace with… and Remove: Remove
/// removes it, and Replace with… opens the palette, edits nothing, and a pick there raises the
/// Replace dialog for the ghost node, not an added node.
#[test]
fn a_ghost_nodes_menu_offers_replace_and_remove() {
    let mut fixture = DocumentFixture::probes(1);
    let ghost = fixture.add_ghost_node(DocumentFixture::missing_node());
    let mut h = CanvasHarness::shaping_text(fixture, SURFACE);
    h.document_mut().main_view.selected = [ghost].map(ItemId::Node).into_iter().collect();

    let rows = open_menu(&mut h, ghost);
    assert_eq!(rows.len(), 2, "Replace with… and Remove");
    h.ui.click(rows[1]);
    let intents = h.frame();
    assert!(
        matches!(intents[..], [GraphIntent::RemoveNode { node_id }] if node_id == ghost),
        "{intents:?}"
    );

    let rows = open_menu(&mut h, ghost);
    h.ui.click(rows[0]);
    assert!(h.frame().is_empty(), "Replace with… edits nothing");
    h.frame();
    assert!(
        h.graph_ui.gestures.new_node_ui.in_flight(),
        "the palette opens"
    );
    h.ui.key(palantir::Key::Enter);
    let intents = h.frame();
    assert!(intents.is_empty(), "{intents:?}");
    assert!(
        matches!(
            h.commands[..],
            [AppCommand::ProposeReplace { node_id, .. }] if node_id == ghost
        ),
        "{:?}",
        h.commands
    );
}

/// A node-body right-click selects the node it landed on before the menu opens, so whatever the
/// user picks next acts on a coherent set. Checked through a real click, as it comes out of the
/// closure the shared trigger scan takes.
#[test]
fn a_node_body_right_click_selects_the_node_it_landed_on() {
    // Two nodes, so the exactly-one-selected assertion has something to exclude.
    let mut h = CanvasHarness::sized(DocumentFixture::probes(2), SURFACE);
    let func = h.node(0);
    // Two frames so every node body has recorded and carries a hit-testable
    // rect for the click below.
    h.prime(2);

    let on_func = h.node_center(func);
    h.ui.right_click(on_func);
    let intents = h.frame();
    assert!(
        matches!(
            intents[..],
            [GraphIntent::SetSelection { ref to }] if to.len() == 1 && to.contains(&ItemId::Node(func)),
        ),
        "the right-click selects exactly the node it opened on: {intents:?}"
    );
}

/// "Remove" acts on the whole selection, not only the node the menu opened on: one intent per
/// member, which the drain batches into a single undo entry. Opening on a node already in the
/// selection leaves that selection alone, so both members survive to the pick.
#[test]
fn remove_pick_removes_every_selected_node() {
    let mut h = CanvasHarness::shaping_text(DocumentFixture::probes(3), SURFACE);
    let (a, b) = (h.node(0), h.node(1));
    h.document_mut().main_view.selected = [a, b].map(ItemId::Node).into_iter().collect();

    let intents = pick(&mut h, a, REMOVE);
    let removed: BTreeSet<NodeId> = intents
        .iter()
        .filter_map(|intent| match intent {
            GraphIntent::RemoveNode { node_id } => Some(*node_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        removed,
        BTreeSet::from([a, b]),
        "both selected nodes go, the third stays: {intents:?}"
    );
    assert_eq!(intents.len(), 2, "and nothing rides along: {intents:?}");
}

/// The two Duplicate rows differ in exactly one thing: whether a clone keeps
/// its wire to a producer outside the selection. With `c -> b` crossing that
/// boundary, the plain pick drops the wire and the "with incoming" pick keeps
/// it pointed at the original `c`.
#[test]
fn duplicate_picks_differ_by_whether_incoming_wires_survive() {
    /// How many bindings the `row`th pick's clones are seeded with, the first
    /// of them, and the `c` it may point at.
    #[derive(Debug)]
    struct Seeded {
        count: usize,
        first: Option<Binding>,
        c: NodeId,
    }
    let duplicate_via = |row: usize| {
        let mut h = CanvasHarness::shaping_text(DocumentFixture::probes(3), SURFACE);
        let (a, b, c) = (h.node(0), h.node(1), h.node(2));
        // The one wire crossing the selection boundary: b reads c, and only
        // {a, b} are selected.
        h.document_mut().graphs.main_mut().set_input_binding(
            InputPort::new(b, PortId::new(1)),
            Binding::wire(c, PortId::new(1)),
        );
        h.document_mut().main_view.selected = [a, b].map(ItemId::Node).into_iter().collect();

        // Duplicating expands into ordinary intents: one add per clone, the
        // surviving wiring seeded onto the clone that consumes it, then the
        // selection swap that leaves the copies selected.
        let intents = pick(&mut h, a, row);
        let seeded: Vec<Binding> = intents
            .iter()
            .filter_map(|intent| match intent {
                GraphIntent::AddNode { bindings, .. } => Some(bindings),
                _ => None,
            })
            .flat_map(|bindings| bindings.iter().map(|entry| entry.binding.clone()))
            .collect();
        assert_eq!(
            intents
                .iter()
                .filter(|intent| matches!(intent, GraphIntent::AddNode { .. }))
                .count(),
            2,
            "the two selected nodes are cloned: {intents:?}"
        );
        assert!(
            matches!(intents.last(), Some(GraphIntent::SetSelection { .. })),
            "and the copies end up selected: {intents:?}"
        );
        Seeded {
            count: seeded.len(),
            first: seeded.into_iter().next(),
            c,
        }
    };

    let dropped = duplicate_via(DUPLICATE);
    let Seeded { count, first, c } = duplicate_via(DUPLICATE_WITH_INCOMING);
    assert_eq!(dropped.count, 0, "the external wire is dropped");
    assert_eq!(count, 1, "the external wire is kept");
    match first {
        Some(Binding::Wire(src)) => {
            assert_eq!(src.node_id, c, "still fed by the original producer");
            assert_eq!(src.port, PortId::new(1));
        }
        other => panic!("expected a Bind to c, got {other:?}"),
    }
}

/// "Run to this node" is the one pick that names the node the menu opened on rather than the
/// selection, and the one that leaves the graph alone: it asks `App` to run that node.
#[test]
fn run_pick_names_the_opened_node_and_edits_nothing() {
    let mut h = CanvasHarness::shaping_text(DocumentFixture::probes(2), SURFACE);
    let (a, b) = (h.node(0), h.node(1));
    // A different node selected, so "the node the menu opened on" and "the
    // selection" name different things and the assertion can tell them apart.
    h.document_mut().main_view.selected = [b].map(ItemId::Node).into_iter().collect();

    let intents = pick(&mut h, a, RUN);
    assert!(
        intents.is_empty(),
        "running a node edits nothing: {intents:?}"
    );
    assert!(
        matches!(
            h.commands[..],
            [AppCommand::Run(RunCommand::Node(program_id))] if program_id == ProgramId::from(a),
        ),
        "the run names the node the menu opened on, not the selection: {:?}",
        h.commands
    );
}

/// A menu whose node the document drops while it is open, as a Delete or an undo does, closes:
/// its picks would apply to nothing, and Remove to whatever the selection is.
#[test]
fn a_menu_closes_when_its_node_leaves_the_document() {
    let mut h = CanvasHarness::shaping_text(DocumentFixture::probes(2), SURFACE);
    let (gone, stays) = (h.node(0), h.node(1));
    open_menu(&mut h, gone);
    assert!(h.graph_ui.gestures.node_menu.in_flight(), "the menu opened");

    h.document_mut().graphs.main_mut().detach_node(gone);
    h.document_mut().main_view.node_placements.remove(&gone);
    h.document_mut().main_view.selected = [stays].map(ItemId::Node).into_iter().collect();
    assert!(h.frame().is_empty(), "the closing frame edits nothing");
    assert!(
        !h.graph_ui.gestures.node_menu.in_flight(),
        "the menu closed"
    );
}

/// An instance's menu offers its compound's actions after the duplicates: Open, Expand, Make
/// Unique while another node places it too, Make Local while it follows the library, and Lock,
/// which reads Unlock on a locked compound. Open enters the compound through the
/// instance, and Lock, Unlock and Make Local raise their edit of the compound.
#[test]
fn an_instances_menu_offers_its_compounds_actions() {
    use scenarium::{Compound, CompoundId, InstancePath, Interface};

    use crate::core::document::internals::insert_compound;
    use crate::core::edit::compound_intent::CompoundIntent;

    let mut fixture = DocumentFixture::probes(0);
    let id = CompoundId::from_u128(1);
    insert_compound(
        &mut fixture.document,
        id,
        Compound::new("inner", Interface::default()),
    );
    let instance = fixture.add_instance(id);
    let other = fixture.add_instance(id);
    let mut h = CanvasHarness::shaping_text(fixture, SURFACE);
    let texts = |h: &mut CanvasHarness| {
        let rows = open_menu(h, instance);
        let widgets = h.ui.widgets(None);
        let text = |id: &WidgetId| {
            widgets
                .iter()
                .find(|widget| widget.id == *id)
                .map_or_else(String::new, |widget| widget.text.clone())
        };
        rows.iter().map(|row| (text(row), *row)).collect::<Vec<_>>()
    };
    let labels = |rows: &[(String, WidgetId)]| {
        rows.iter()
            .map(|(text, _)| text.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let click = |h: &mut CanvasHarness, rows: &[(String, WidgetId)], label: &str| {
        let (_, row) = rows
            .iter()
            .find(|(text, _)| text == label)
            .unwrap_or_else(|| panic!("{label} is offered: {}", labels(rows)));
        h.ui.click(*row);
        h.frame()
    };

    let rows = texts(&mut h);
    assert_eq!(
        labels(&rows),
        "Duplicate, Duplicate with incoming wires, Open, Expand, Make Unique, Lock, \
         Replace with\u{2026}, Remove"
    );
    click(&mut h, &rows, "Open");
    assert_eq!(h.entries, [(id, Some(InstancePath::new(&[instance])))]);
    let rows = texts(&mut h);
    let expanded = click(&mut h, &rows, "Expand");
    assert!(
        expanded.iter().any(
            |intent| matches!(intent, GraphIntent::RemoveNode { node_id } if *node_id == instance)
        ),
        "Expand takes the instance out for its nodes: {expanded:?}"
    );
    let rows = texts(&mut h);
    click(&mut h, &rows, "Lock");
    assert!(
        matches!(h.compound_edits[..], [CompoundIntent::SetLocked { id: locked, locked: true }] if locked == id),
        "{:?}",
        h.compound_edits
    );

    h.document_mut().sides.get_mut(&id).unwrap().locked = true;
    let rows = texts(&mut h);
    click(&mut h, &rows, "Unlock");
    assert!(
        matches!(h.compound_edits[..], [CompoundIntent::SetLocked { id: locked, locked: false }] if locked == id),
        "{:?}",
        h.compound_edits
    );

    let side = h.document_mut().sides.get_mut(&id).unwrap();
    (side.locked, side.follows) = (false, true);
    let rows = texts(&mut h);
    assert_eq!(
        labels(&rows),
        "Duplicate, Duplicate with incoming wires, Open, Expand, Make Unique, Make Local, Lock, \
         Replace with\u{2026}, Remove"
    );
    click(&mut h, &rows, "Make Local");
    assert!(
        matches!(h.compound_edits[..], [CompoundIntent::SetFollows { id: local, follows: false }] if local == id),
        "{:?}",
        h.compound_edits
    );

    h.document_mut().remove_node(GraphId::Main, other);
    let rows = texts(&mut h);
    assert!(
        !labels(&rows).contains("Make Unique"),
        "one node places it: {}",
        labels(&rows)
    );
}
