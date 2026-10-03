use scenarium::NodeId;
use std::iter;

use super::*;
use crate::core::edit::document_request::DocumentRequest;
use crate::gui::app::commands::run::RunCommand;

fn remove_node() -> GraphIntent {
    GraphIntent::RemoveNode {
        node_id: NodeId::unique(),
    }
}

fn close_prefs() -> DockOp<TabRef> {
    DockOp::CloseTab {
        tab: TabRef::Preferences,
    }
}

/// The document drain takes both of its tiers in the order raised and
/// leaves the app tier untouched — which is what lets the editor drain
/// three times a frame while `App` still gets every command, in order,
/// once the pass is over. Pinned here because the routing lives in the
/// push methods: a tier pushed to the wrong queue would surface as a
/// command vanishing mid-pass rather than as a type error.
#[test]
fn each_level_drains_its_own_tier_and_leaves_the_rest_queued() {
    let mut out = Requests::default();
    out.push_graph(remove_node());
    out.push_app(AppCommand::Run(RunCommand::Once));
    out.push_view(close_prefs());
    out.push_app(AppCommand::Quit);
    out.extend_graph([remove_node(), remove_node()]);

    let first: Vec<&str> = out
        .document()
        .drain()
        .map(|item| match item {
            DocumentRequest::Graph(_) => "graph",
            DocumentRequest::View(_) => "view",
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
