use std::collections::BTreeSet;

use glam::UVec2;
use palantir::WidgetId;
use scenarium::{Binding, GraphId, InputPort, NodeId, PortId, ProgramId};

use crate::core::document::internals::DocumentFixture;
use crate::core::document::item_id::ItemId;
use crate::core::document::pane_kind::PaneKind;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::command::Command;
use crate::gui::command::menu::{MenuRow, NODE_MENU};
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::RunCommand;
use crate::gui::frame::pane_ask::PaneAsk;
use crate::gui::pane::graph_ui::internals::CanvasHarness;

/// Room for three nodes in a row *and* the opened menu below them.
const SURFACE: UVec2 = UVec2::new(1600, 900);

/// What `command`'s row reads: its label, then its chord.
fn row_text(command: Command) -> String {
    let label = command.spec().label;
    match command.keys() {
        Some(keys) => format!("{label} {keys}"),
        None => label.to_owned(),
    }
}

/// The commands of [`NODE_MENU`], in its order.
fn commands() -> impl Iterator<Item = Command> {
    NODE_MENU.iter().filter_map(|row| match *row {
        MenuRow::Command(command) => Some(command),
        MenuRow::Separator | MenuRow::RecentDocuments => None,
    })
}

/// The open menu's row of `command`, if it offers one.
fn row(h: &CanvasHarness, command: Command) -> Option<WidgetId> {
    let text = row_text(command);
    h.ui.widgets(Some(&text))
        .into_iter()
        .find(|widget| widget.text == text)
        .map(|widget| widget.id)
}

/// The commands the open menu offers, in the table's order.
fn offered(h: &CanvasHarness) -> Vec<Command> {
    commands()
        .filter(|&command| row(h, command).is_some())
        .collect()
}

/// Open the menu on `on`. The open and a pick are separate frames because the menu has to record
/// before an item can be hit, as in the app.
fn open_menu(h: &mut CanvasHarness, on: NodeId) {
    // Two frames so every node body has recorded and carries a hit-testable
    // rect to aim the right-click at, then two more so the opened menu has.
    h.prime(2);
    let anchor = h.node_center(on);
    h.ui.right_click(anchor);
    h.prime(2);
}

/// Open the menu on `on`, click `command`'s row, and return the intents the picking frame
/// raised.
fn pick(h: &mut CanvasHarness, on: NodeId, command: Command) -> Vec<GraphIntent> {
    open_menu(h, on);
    let row = row(h, command).unwrap_or_else(|| panic!("{command:?} is offered: {:?}", offered(h)));
    h.ui.click(row);
    h.frame()
}

/// A ghost node neither runs nor copies, so its menu offers Delete, Replace with… and the
/// inspector: Delete removes it, and Replace with… asks its pane for the palette, edits nothing,
/// and a pick there raises the Replace dialog for the ghost node, not an added node.
#[test]
fn a_ghost_nodes_menu_offers_delete_and_replace() {
    let mut fixture = DocumentFixture::probes(1);
    let ghost = fixture.add_ghost_node(DocumentFixture::missing_node());
    let mut h = CanvasHarness::shaping_text(fixture, SURFACE);
    h.document_mut().main_view.selected = [ghost].map(ItemId::Node).into_iter().collect();

    open_menu(&mut h, ghost);
    assert_eq!(
        offered(&h),
        [
            Command::Delete,
            Command::ReplaceNode,
            Command::ShowPane(PaneKind::Inspector)
        ]
    );
    let intents = pick(&mut h, ghost, Command::Delete);
    assert!(
        matches!(intents[..], [GraphIntent::RemoveNode { node_id }] if node_id == ghost),
        "{intents:?}"
    );

    let intents = pick(&mut h, ghost, Command::ReplaceNode);
    assert!(intents.is_empty(), "Replace with… edits nothing");
    assert!(
        matches!(
            h.commands[..],
            [AppCommand::Pane { graph: GraphId::Main, ask: PaneAsk::Replace(node_id) }]
                if node_id == ghost
        ),
        "{:?}",
        h.commands
    );
    // As `App` hands the ask to the pane.
    h.graph_ui.ask(PaneAsk::Replace(ghost));
    h.frame();
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

/// Delete acts on the whole selection, not only the node the menu opened on: one intent per
/// member, which the drain batches into a single undo entry. Opening on a node already in the
/// selection leaves that selection alone, so both members survive to the pick.
#[test]
fn delete_pick_removes_every_selected_node() {
    let mut h = CanvasHarness::shaping_text(DocumentFixture::probes(3), SURFACE);
    let (a, b) = (h.node(0), h.node(1));
    h.document_mut().main_view.selected = [a, b].map(ItemId::Node).into_iter().collect();

    let intents = pick(&mut h, a, Command::Delete);
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
    /// How many bindings a pick's clones are seeded with, the first
    /// of them, and the `c` it may point at.
    #[derive(Debug)]
    struct Seeded {
        count: usize,
        first: Option<Binding>,
        c: NodeId,
    }
    let duplicate_via = |command: Command| {
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
        let intents = pick(&mut h, a, command);
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

    let dropped = duplicate_via(Command::Duplicate);
    let Seeded { count, first, c } = duplicate_via(Command::DuplicateWithWires);
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

/// Run to Selected Node runs the node the menu opened on, which the right-click selected, and
/// leaves the graph alone: it asks `App` to run that node.
#[test]
fn run_pick_names_the_opened_node_and_edits_nothing() {
    let mut h = CanvasHarness::shaping_text(DocumentFixture::probes(2), SURFACE);
    let (a, b) = (h.node(0), h.node(1));
    h.document_mut().main_view.selected = [b].map(ItemId::Node).into_iter().collect();

    open_menu(&mut h, a);
    // The right-click's selection edit, which the session's drain lands before a pick.
    h.document_mut().main_view.selected = [a].map(ItemId::Node).into_iter().collect();
    h.prime(1);
    let row = row(&h, Command::RunToSelection).expect("a probe runs");
    h.ui.click(row);
    let intents = h.frame();
    assert!(
        intents.is_empty(),
        "running a node edits nothing: {intents:?}"
    );
    assert!(
        matches!(
            h.commands[..],
            [AppCommand::Run(RunCommand::Node(program_id))] if program_id == ProgramId::from(a),
        ),
        "the run names the node the menu opened on: {:?}",
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

/// An instance's menu offers its compound's commands after the node's own: Open, Expand, Make
/// Unique while another node places it too, Make Local while it follows the library, and Locked,
/// checked on a locked compound. Open enters the compound through the instance, and Locked and
/// Make Local raise their edit of the compound.
#[test]
fn an_instances_menu_offers_its_compounds_commands() {
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
    h.document_mut().main_view.selected = [instance].map(ItemId::Node).into_iter().collect();
    let node = [
        Command::Cut,
        Command::Copy,
        Command::Duplicate,
        Command::DuplicateWithWires,
        Command::Delete,
        Command::Rename,
        Command::ReplaceNode,
        Command::CollapseToCompound,
    ];
    let with = |compound: &[Command]| {
        let mut all = node.to_vec();
        all.extend_from_slice(compound);
        all.push(Command::ShowPane(PaneKind::Inspector));
        all
    };

    open_menu(&mut h, instance);
    assert_eq!(
        offered(&h),
        with(&[
            Command::OpenCompound,
            Command::ExpandCompound,
            Command::MakeUnique,
            Command::ToggleLock
        ])
    );
    pick(&mut h, instance, Command::OpenCompound);
    assert_eq!(h.entries, [(id, Some(InstancePath::new(&[instance])))]);
    let expanded = pick(&mut h, instance, Command::ExpandCompound);
    assert!(
        expanded.iter().any(
            |intent| matches!(intent, GraphIntent::RemoveNode { node_id } if *node_id == instance)
        ),
        "Expand takes the instance out for its nodes: {expanded:?}"
    );
    pick(&mut h, instance, Command::ToggleLock);
    assert!(
        matches!(h.compound_edits[..], [CompoundIntent::SetLocked { id: locked, locked: true }] if locked == id),
        "{:?}",
        h.compound_edits
    );

    h.document_mut().sides.get_mut(&id).unwrap().locked = true;
    pick(&mut h, instance, Command::ToggleLock);
    assert!(
        matches!(h.compound_edits[..], [CompoundIntent::SetLocked { id: locked, locked: false }] if locked == id),
        "{:?}",
        h.compound_edits
    );

    let side = h.document_mut().sides.get_mut(&id).unwrap();
    (side.locked, side.follows) = (false, true);
    open_menu(&mut h, instance);
    // A graph that follows the library takes no edit, so Make Unique waits for Make Local.
    assert_eq!(
        offered(&h),
        with(&[
            Command::OpenCompound,
            Command::ExpandCompound,
            Command::MakeUnique,
            Command::MakeLocal,
            Command::ToggleLock
        ])
    );
    pick(&mut h, instance, Command::MakeLocal);
    assert!(
        matches!(h.compound_edits[..], [CompoundIntent::SetFollows { id: local, follows: false }] if local == id),
        "{:?}",
        h.compound_edits
    );

    h.document_mut().remove_node(GraphId::Main, other);
    open_menu(&mut h, instance);
    assert!(
        !offered(&h).contains(&Command::MakeUnique),
        "one node places it: {:?}",
        offered(&h)
    );
}
