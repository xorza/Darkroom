use palantir::DockOperation;
use scenarium::NodeId;

use crate::core::document::internals::DocumentFixture;
use crate::core::document::item_id::ItemId;
use crate::core::document::pane_kind::PaneKind;
use crate::core::document::tab_ref::TabRef;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::app::session::internals::SessionHarness;
use crate::gui::command::{Command, Scope};
use crate::gui::frame::app_command::{AppCommand, FileCommand};

/// A command's place in the enum, a pane's once a kind. Exhaustive, so a new command does not
/// compile until it has a place, which the test below holds [`Command::ALL`] to.
const fn ordinal(command: Command) -> usize {
    match command {
        Command::NewDocument => 0,
        Command::OpenDocument => 1,
        Command::SaveDocument => 2,
        Command::SaveDocumentAs => 3,
        Command::PackResources => 4,
        Command::Quit => 5,
        Command::Undo => 6,
        Command::Redo => 7,
        Command::Cut => 8,
        Command::Copy => 9,
        Command::Paste => 10,
        Command::Duplicate => 11,
        Command::DuplicateWithWires => 12,
        Command::Delete => 13,
        Command::SelectAll => 14,
        Command::Rename => 15,
        Command::ZoomIn => 16,
        Command::ZoomOut => 17,
        Command::ZoomToActual => 18,
        Command::ZoomToFit => 19,
        Command::ZoomToSelection => 20,
        Command::ShowPane(PaneKind::Viewer) => 21,
        Command::ShowPane(PaneKind::Inspector) => 22,
        Command::ShowPane(PaneKind::Preferences) => 23,
        Command::ShowPane(PaneKind::Reference) => 24,
        Command::CloseTab => 25,
        Command::CopyViewImage => 26,
        Command::SaveViewImage => 27,
        Command::Run => 28,
        Command::CancelRun => 29,
        Command::RunToSelection => 30,
        Command::ToggleEvents => 31,
        Command::ToggleRunOnChange => 32,
        Command::AddNode => 33,
        Command::ReplaceNode => 34,
        Command::OpenCompound => 35,
        Command::CollapseToCompound => 36,
        Command::ExpandCompound => 37,
        Command::MakeUnique => 38,
        Command::MakeLocal => 39,
        Command::ToggleLock => 40,
        Command::ExportCompound => 41,
        Command::SelectGhosts => 42,
        Command::RemoveGhosts => 43,
        Command::RemoveUnusedCompounds => 44,
    }
}

/// `ALL` holds every command once, in the enum's order: the last ordinal is its last index.
#[test]
fn all_lists_every_command_in_order() {
    for (idx, command) in Command::ALL.into_iter().enumerate() {
        assert_eq!(ordinal(command), idx, "{command:?}");
    }
    assert_eq!(
        Command::ALL.len(),
        ordinal(Command::RemoveUnusedCompounds) + 1
    );
}

/// No two commands share a chord, Backspace, Delete's second key, included.
#[test]
fn no_two_commands_share_a_chord() {
    let mut seen: Vec<String> = vec!["Backspace".to_owned()];
    for command in Command::ALL {
        let Some(keys) = command.keys() else {
            continue;
        };
        let label = format!("{keys:?}");
        assert!(!seen.contains(&label), "{command:?} repeats {label}");
        seen.push(label);
    }
    // File 5, the history 2, the selection's 7, the camera 5, Preferences, Close Tab, Run and
    // Add Node, then Backspace.
    assert_eq!(seen.len(), 5 + 2 + 7 + 5 + 4 + 1);
}

fn nodes(h: &SessionHarness) -> Vec<NodeId> {
    h.session
        .open
        .document()
        .graphs
        .main()
        .iter()
        .map(|node| node.node_id)
        .collect()
}

fn select(h: &mut SessionHarness, node_id: NodeId) {
    h.apply(GraphIntent::SetSelection {
        to: [ItemId::Node(node_id)].into(),
    });
}

/// A chord of the graph's scope waits for a pane that shows the active graph: with Preferences
/// in front of it, Delete removes nothing, and a chord of the window's scope still fires.
#[test]
fn a_graph_chord_waits_for_a_pane_that_shows_the_graph() {
    assert_eq!(Command::Delete.spec().scope, Scope::Graph);
    assert_eq!(Command::SaveDocument.spec().scope, Scope::Window);
    let mut h = SessionHarness::new(DocumentFixture::probes(2));
    let before = nodes(&h);
    select(&mut h, before[0]);
    h.requests.push_view(DockOperation::OpenTab {
        tab: TabRef::Preferences,
    });
    h.drain();
    h.prime(2);

    h.ui.key(Command::Delete.chord_keys());
    assert!(h.frame().is_empty());
    assert_eq!(nodes(&h), before, "nobody is looking at the graph");

    h.ui.key(Command::SaveDocument.chord_keys());
    let commands = h.frame();
    assert!(
        matches!(commands[..], [AppCommand::File(FileCommand::Save)]),
        "{commands:?}"
    );

    h.requests.push_view(DockOperation::ActivateTab {
        tab: TabRef::Graph(scenarium::GraphId::Main),
    });
    h.drain();
    h.prime(2);
    h.ui.key(Command::Delete.chord_keys());
    h.frame();
    assert_eq!(nodes(&h), before[1..], "the selected node went");
}

/// Select All takes every node; Cut removes the selection and leaves it on the clipboard, which
/// Paste then puts back as a new node.
#[test]
fn select_all_cut_and_paste() {
    let mut h = SessionHarness::new(DocumentFixture::probes(3));
    let before = nodes(&h);
    h.prime(2);

    h.ui.key(Command::SelectAll.chord_keys());
    h.frame();
    let selected = &h.session.open.document().main_view.selected;
    let all = before.iter().copied().map(ItemId::Node).collect();
    assert_eq!(*selected, all);

    select(&mut h, before[1]);
    h.prime(1);
    h.ui.key(Command::Cut.chord_keys());
    h.frame();
    assert_eq!(nodes(&h), [before[0], before[2]], "the cut node went");

    h.ui.key(Command::Paste.chord_keys());
    h.frame();
    let after = nodes(&h);
    assert_eq!(after.len(), 3, "the clip came back");
    assert!(!after.contains(&before[1]), "under an id of its own");
}

/// A pane's row is checked while its tab is open, and Close Tab closes the focused group's
/// visible tab, the pinned main graph's excepted.
#[test]
fn a_pane_shows_checked_and_close_tab_closes_it() {
    use crate::gui::command::Availability;
    use crate::gui::frame::app_ctx::AppCtx;
    use crate::gui::frame::doc_ctx::DocCtx;
    use crate::gui::graph_ctx::GraphCtx;

    let mut h = SessionHarness::new(DocumentFixture::probes(1));
    let availability = |h: &mut SessionHarness, command: Command| {
        let display = h.ui.ui_mut().display();
        let app = AppCtx::new(
            &h.theme,
            &h.library,
            &h.run_state,
            &h.shared,
            h.preferences.run_mode,
        );
        let doc = DocCtx::new(app, &h.session.open);
        command.availability(GraphCtx::of(doc, doc.active(), display))
    };
    let reference = Command::ShowPane(PaneKind::Reference);
    h.prime(2);
    assert_eq!(availability(&mut h, reference), Availability::Available);
    assert_eq!(
        availability(&mut h, Command::CloseTab),
        Availability::Unavailable,
        "the main graph's tab is pinned"
    );

    h.ui.key(Command::ShowPane(PaneKind::Preferences).chord_keys());
    h.frame();
    let tabs = |h: &SessionHarness| -> Vec<TabRef> {
        h.session.open.document().layout.all_tabs().collect()
    };
    assert!(tabs(&h).contains(&TabRef::Preferences));
    assert_eq!(
        availability(&mut h, Command::ShowPane(PaneKind::Preferences)),
        Availability::On
    );

    h.ui.key(Command::CloseTab.chord_keys());
    h.frame();
    assert_eq!(tabs(&h), [TabRef::Graph(scenarium::GraphId::Main)]);
}
