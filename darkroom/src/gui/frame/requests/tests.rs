use glam::UVec2;
use palantir::Harness;
use scenarium::NodeId;
use std::iter;

use super::*;
use crate::core::edit::document_request::DocumentRequest;
use crate::gui::frame::app_command::RunCommand;

fn remove_node() -> GraphIntent {
    GraphIntent::RemoveNode {
        node_id: NodeId::unique(),
    }
}

fn close_preferences() -> DockOperation<TabRef> {
    DockOperation::CloseTab {
        tab: TabRef::Preferences,
    }
}

/// The document drain takes both of its tiers in the order raised and leaves the app tier
/// queued, so the editor can drain three times a frame while `App` still gets every command, in
/// order, after the pass. A tier pushed to the wrong queue would show as a command vanishing.
#[test]
fn each_level_drains_its_own_tier_and_leaves_the_rest_queued() {
    let mut out = Requests::default();
    out.push_graph(GraphId::Main, remove_node());
    out.push_app(AppCommand::Run(RunCommand::Once));
    out.push_view(close_preferences());
    out.push_app(AppCommand::Quit);
    out.extend_graph(GraphId::Main, [remove_node(), remove_node()]);

    let first: Vec<&str> = out
        .document()
        .drain()
        .map(|item| match item {
            DocumentRequest::Graph { .. }
            | DocumentRequest::InGesture { .. }
            | DocumentRequest::Cancel { .. }
            | DocumentRequest::Compound(_) => "graph",
            DocumentRequest::View(_)
            | DocumentRequest::Enter { .. }
            | DocumentRequest::Show(_)
            | DocumentRequest::ShowPreview(_) => "view",
        })
        .collect();
    assert_eq!(
        first,
        ["graph", "view", "graph", "graph"],
        "both document tiers come out interleaved as raised"
    );
    // A second document drain — the editor runs three a frame — finds
    // nothing left of its own and still leaves the app tier alone.
    assert_eq!(out.document().drain().count(), 0);

    let commands: Vec<AppCommand> = iter::from_fn(|| out.pop_app()).collect();
    assert!(
        matches!(
            commands[..],
            [AppCommand::Run(RunCommand::Once), AppCommand::Quit]
        ),
        "the app tier comes out in the order raised: {commands:?}"
    );
    assert!(
        out.document().is_empty(),
        "and the queue is empty once both have drained"
    );
}

/// A debt comes from a direct owe, or from an edit that strands the geometry and from no other, and
/// a spend asks one pass and clears it, however many levels owed it: 1 record for no debt and a
/// move's facts, 2 for a debt, and 1 again past the spend. A run is owed by an edit of what a run
/// reads alone, once however many owed it, apart from the relayout.
#[test]
fn a_relayout_or_a_run_owed_is_spent_once() {
    let mut h = Harness::builder(UVec2::ONE).mono().build();
    let mut out = Requests::default();
    let passes = |h: &mut Harness, out: &mut Requests| {
        let mut passes = 0;
        h.ui_mut().request_repaint();
        h.frame(|ui| {
            passes += 1;
            out.spend_relayout(ui);
        });
        passes
    };
    assert_eq!(passes(&mut h, &mut out), 1, "nothing owed");

    out.owe_after(EditFacts::NONE);
    assert!(!out.owes_relayout(), "a move strands nothing");
    assert!(!out.take_run(), "and changes nothing a run reads");

    out.owe_relayout();
    out.owe_after(EditFacts {
        renamed: true,
        ..EditFacts::NONE
    });
    assert!(out.owes_relayout());
    assert!(!out.take_run(), "a name is not read by a run");
    assert!(out.is_empty(), "a debt is no request");
    assert_eq!(passes(&mut h, &mut out), 2, "two debts, one pass");
    assert!(!out.owes_relayout());
    assert_eq!(passes(&mut h, &mut out), 1, "spent");

    let program = EditFacts {
        program: true,
        ..EditFacts::NONE
    };
    out.owe_after(program);
    out.owe_after(program);
    assert!(!out.owes_relayout(), "a run owed strands no geometry");
    assert!(out.take_run(), "two edits owe one run");
    assert!(!out.take_run(), "taken");
}
