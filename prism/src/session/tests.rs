use super::*;
use crate::model::ViewNode;
use crate::ui_host::UiHost;
use egui::Pos2;

#[derive(Debug)]
struct NoopUiHost;
impl UiHost for NoopUiHost {
    fn request_redraw(&self) {}
    fn close_app(&self) {}
}

/// Stub-backed Session for unit tests: no tokio runtime, no
/// network listener, no config autoload.
fn test_session() -> Session {
    test_session_with(FuncLib::default(), unbounded_channel::<SessionInbound>().1)
}

/// Like `test_session` but lets the test inject a populated FuncLib
/// and a script-action receiver so it can drain script side-effects.
fn test_session_with(
    func_lib: FuncLib,
    script_inbound_rx: UnboundedReceiver<SessionInbound>,
) -> Session {
    let (worker_tx, worker_rx) = unbounded_channel::<WorkerEvent>();
    Session::from_parts(
        Arc::new(func_lib),
        Config::default(),
        None,
        worker_tx,
        worker_rx,
        None,
        script_inbound_rx,
        Arc::new(NoopUiHost),
    )
}

#[test]
fn add_status_first_line_has_no_leading_newline() {
    let mut session = test_session();
    session.add_status("hello");
    assert_eq!(session.status(), "hello");
}

#[test]
fn add_status_appends_with_newline_separator() {
    let mut session = test_session();
    session.add_status("one");
    session.add_status("two");
    assert_eq!(session.status(), "one\ntwo");
}

#[test]
fn add_status_caps_buffer_to_2000_chars() {
    let mut session = test_session();
    for _ in 0..300 {
        session.add_status("0123456789");
    }
    assert!(session.status().len() <= STATUS_CAP);
}

#[test]
fn drain_inbound_apply_node_added_inserts_node_and_marks_dirty() {
    use scenarium::function::{Func, FuncId};
    use scenarium::graph::Node;

    let alpha_id = FuncId::unique();
    let func = Func {
        id: alpha_id,
        name: "alpha".to_string(),
        ..Default::default()
    };
    let mut lib = FuncLib::default();
    lib.add(func.clone());
    let (script_tx, script_rx) = unbounded_channel::<SessionInbound>();
    let mut session = test_session_with(lib, script_rx);
    // Clear the dirty flag set by Session::from_parts so we can
    // observe drain_inbound flipping it back on.
    session.graph_dirty = false;

    // Sanity: graph starts empty.
    assert_eq!(session.view_graph.graph.len(), 0);
    assert_eq!(session.view_graph.view_nodes.iter().count(), 0);

    // Build the same action the script executor would produce.
    let pos = Pos2::new(12.0, 34.0);
    let node: Node = (&func).into();
    let node_id = node.id;
    let view_node = ViewNode { id: node_id, pos };
    script_tx
        .send(SessionInbound::Apply(vec![GraphUiAction::AddNode {
            view_node,
            node,
        }]))
        .unwrap();
    session.drain_inbound();

    assert_eq!(session.view_graph.graph.len(), 1);
    assert_eq!(session.view_graph.view_nodes.iter().count(), 1);
    let stored = session.view_graph.graph.iter().next().unwrap();
    assert_eq!(stored.id, node_id);
    assert_eq!(stored.name, "alpha");
    assert_eq!(
        session.view_graph.view_nodes.by_key(&node_id).unwrap().pos,
        pos
    );
    // AddNode affects computation, so the dirty flag must trip.
    assert!(session.graph_dirty);
}

#[test]
fn apply_reports_when_action_affects_computation() {
    let mut session = test_session();

    // SelectNode is a UI-only action — should NOT affect computation.
    let affects = session.apply(&[GraphUiAction::SelectNode {
        before: None,
        after: None,
    }]);
    assert!(!affects);

    // MoveNode is also UI-only.
    let node_id = NodeId::unique();
    session.view_graph.view_nodes.add(ViewNode {
        id: node_id,
        pos: Pos2::ZERO,
    });
    let affects = session.apply(&[GraphUiAction::MoveNode {
        node_id,
        before: Pos2::ZERO,
        after: Pos2::new(1.0, 2.0),
    }]);
    assert!(!affects);
    assert_eq!(
        session.view_graph.view_nodes.by_key(&node_id).unwrap().pos,
        Pos2::new(1.0, 2.0)
    );
}
