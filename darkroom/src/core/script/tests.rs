use super::*;
use arc_swap::ArcSwap;
use scenarium::graph::{Binding, NodeKind, OutputPort};
use scenarium::prelude::Library;

/// Build an `InboundSender` paired with the receiver tests assert on.
/// `notify` is a no-op — tests don't drive a real host loop.
fn test_inbound() -> (InboundSender, mpsc::UnboundedReceiver<ScriptMessage>) {
    let (tx, rx) = mpsc::unbounded_channel::<ScriptMessage>();
    (
        InboundSender {
            tx,
            notify: Arc::new(|| {}),
        },
        rx,
    )
}

/// Drain the next inbound, asserting it's a single-batch `Apply`, and
/// return the decoded intents.
fn expect_apply(rx: &mut mpsc::UnboundedReceiver<ScriptMessage>) -> Vec<Intent> {
    match rx.try_recv().expect("Apply queued") {
        ScriptMessage::Apply(actions) => actions,
        other => panic!("expected Apply, got {other:?}"),
    }
}

#[test]
fn list_funcs_returns_full_func_objects_in_insertion_order() {
    use scenarium::node::function::{Func, FuncId};

    let mut lib = Library::default();
    lib.add(Func::new(FuncId::unique(), "alpha").category("math"));
    lib.add(Func::new(FuncId::unique(), "beta").category("io"));

    let state = Arc::new(Mutex::new(String::new()));
    let (tx, _rx) = test_inbound();
    let engine = build_engine(state, tx, Arc::new(ArcSwap::from_pointee(lib)));

    // Each entry is a Rhai Map with fields mirroring `Func`. Verify
    // both insertion order and that the per-func subfields round-trip.
    let names: Array = engine.eval("list_funcs().map(|f| f.name)").unwrap();
    let names: Vec<String> = names
        .into_iter()
        .map(|d| d.into_string().unwrap())
        .collect();
    assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);

    let categories: Array = engine.eval("list_funcs().map(|f| f.category)").unwrap();
    let categories: Vec<String> = categories
        .into_iter()
        .map(|d| d.into_string().unwrap())
        .collect();
    assert_eq!(categories, vec!["math".to_string(), "io".to_string()]);

    // `inputs` / `outputs` round-trip as arrays even when empty.
    let inputs_len: i64 = engine.eval("list_funcs()[0].inputs.len").unwrap();
    assert_eq!(inputs_len, 0);
}

#[test]
fn list_funcs_is_empty_when_func_lib_is_empty() {
    let state = Arc::new(Mutex::new(String::new()));
    let (tx, _rx) = test_inbound();
    let engine = build_engine(
        state,
        tx,
        Arc::new(ArcSwap::from_pointee(Library::default())),
    );

    let result: Array = engine.eval("list_funcs()").unwrap();
    assert!(result.is_empty());
}

#[test]
fn create_node_malformed_id_returns_rhai_error_and_no_action() {
    // `create_node` lives in prelude.rhai; it calls `make_add_node`,
    // which surfaces this error. Confirms the prelude → native helper
    // call chain propagates errors cleanly.
    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    let engine = build_engine(
        state,
        tx,
        Arc::new(ArcSwap::from_pointee(Library::default())),
    );

    let err = engine
        .eval::<String>(r#"create_node("not-a-uuid", 0.0, 0.0)"#)
        .expect_err("malformed id should error");
    assert!(err.to_string().contains("invalid func id"), "got: {err}");
    assert!(rx.try_recv().is_err());
}

#[test]
fn create_node_unknown_id_returns_rhai_error_and_no_action() {
    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    // Empty Library → any well-formed UUID is "unknown".
    let engine = build_engine(
        state,
        tx,
        Arc::new(ArcSwap::from_pointee(Library::default())),
    );

    let err = engine
        .eval::<String>(r#"create_node("00000000-0000-0000-0000-000000000001", 0.0, 0.0)"#)
        .expect_err("unknown id should error");
    assert!(err.to_string().contains("unknown func id"), "got: {err}");
    assert!(rx.try_recv().is_err());
}

#[test]
fn create_node_known_id_enqueues_add_node() {
    use scenarium::node::function::{Func, FuncId};

    let alpha_id = FuncId::unique();
    let mut lib = Library::default();
    lib.add(Func::new(alpha_id, "alpha"));

    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    let engine = build_engine(state, tx, Arc::new(ArcSwap::from_pointee(lib)));

    let script = format!(r#"create_node("{alpha_id}", 12.5, -3.0)"#);
    let returned_id: String = engine.eval(&script).unwrap();

    // The executor builds a fully-formed `Intent::AddNode` — identical to
    // what the new-node popup emits — and ships it via `Apply`. The editor
    // applies it through the same intent/undo path with no script glue.
    let actions = expect_apply(&mut rx);
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Intent::AddNode {
            view_node,
            node,
            def,
            bindings,
        } => {
            assert!(bindings.is_empty(), "script-created nodes seed no defaults");
            assert_eq!(node.kind, NodeKind::Func(alpha_id));
            assert_eq!(node.name, "alpha");
            assert_eq!(view_node.id, node.id);
            assert_eq!(view_node.pos, Vec2::new(12.5, -3.0));
            assert!(def.is_none(), "func nodes carry no subgraph def");
            // The id `create_node` returned to Rhai matches the node id.
            assert_eq!(returned_id, node.id.to_string());
        }
        other => panic!("expected AddNode, got {other:?}"),
    }
}

#[test]
fn apply_decodes_arbitrary_intent_via_serde() {
    // `SetSelection` has a simple shape (a set of ids) and exercises the
    // generic `serde::Deserialize` path that lights up every other variant
    // for free. If this works, a script can drive any current or future
    // `Intent` through `apply` without touching the executor.
    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    let engine = build_engine(
        state,
        tx,
        Arc::new(ArcSwap::from_pointee(Library::default())),
    );

    engine
        .eval::<()>(r#"apply(#{ SetSelection: #{ to: [] } })"#)
        .unwrap();

    let actions = expect_apply(&mut rx);
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Intent::SetSelection { to } => assert!(to.is_empty()),
        other => panic!("expected SetSelection, got {other:?}"),
    }
}

#[test]
fn apply_returns_rhai_error_on_unknown_variant() {
    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    let engine = build_engine(
        state,
        tx,
        Arc::new(ArcSwap::from_pointee(Library::default())),
    );

    let err = engine
        .eval::<()>(r#"apply(#{ NotARealVariant: #{} })"#)
        .expect_err("unknown variant should error");
    assert!(err.to_string().contains("decode Intent"), "got: {err}");
    assert!(rx.try_recv().is_err());
}

#[test]
fn apply_all_batches_actions_into_one_inbound() {
    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    let engine = build_engine(
        state,
        tx,
        Arc::new(ArcSwap::from_pointee(Library::default())),
    );

    // Two no-op selections. Verifies that a Rhai array round-trips into a
    // single `Apply(Vec<...>)` — the path that gives scripts atomic
    // multi-action undo.
    engine
        .eval::<()>(
            r#"apply_all([
                #{ SetSelection: #{ to: [] } },
                #{ SetSelection: #{ to: [] } },
            ])"#,
        )
        .unwrap();

    let actions = expect_apply(&mut rx);
    assert_eq!(actions.len(), 2);
    // No second message — the batch was a single Inbound.
    assert!(rx.try_recv().is_err());
}

#[test]
fn prelude_connect_decodes_to_setinput_bind() {
    // Pins the rewritten `connect` map shape against the new `Intent`:
    // `SetInput { to: Binding::Bind(OutputPort { node_id, port_idx }) }`.
    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    let engine = build_engine(
        state,
        tx,
        Arc::new(ArcSwap::from_pointee(Library::default())),
    );

    let out = NodeId::unique();
    let inp = NodeId::unique();
    engine
        .eval::<()>(&format!(r#"connect("{out}", 2, "{inp}", 1)"#))
        .unwrap();

    let actions = expect_apply(&mut rx);
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        Intent::SetInput {
            node_id,
            input_idx,
            to,
        } => {
            assert_eq!(*node_id, inp);
            assert_eq!(*input_idx, 1);
            match to {
                Binding::Bind(OutputPort { node_id, port_idx }) => {
                    assert_eq!(*node_id, out);
                    assert_eq!(*port_idx, 2);
                }
                other => panic!("expected Bind, got {other:?}"),
            }
        }
        other => panic!("expected SetInput, got {other:?}"),
    }
}

#[test]
fn prelude_disconnect_decodes_to_setinput_none() {
    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    let engine = build_engine(
        state,
        tx,
        Arc::new(ArcSwap::from_pointee(Library::default())),
    );

    let inp = NodeId::unique();
    engine
        .eval::<()>(&format!(r#"disconnect("{inp}", 0)"#))
        .unwrap();

    let actions = expect_apply(&mut rx);
    match &actions[0] {
        Intent::SetInput { to, .. } => assert!(matches!(to, Binding::None)),
        other => panic!("expected SetInput, got {other:?}"),
    }
}

#[test]
fn prelude_move_node_decodes_to_movenodes() {
    // `host::make_move_node` builds the intent in Rust and round-trips it
    // through Rhai → serde_json → `Intent`, so this also pins that
    // `glam::Vec2` survives the round-trip intact.
    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    let engine = build_engine(
        state,
        tx,
        Arc::new(ArcSwap::from_pointee(Library::default())),
    );

    let id = NodeId::unique();
    engine
        .eval::<()>(&format!(r#"move_node("{id}", 5.0, -6.5)"#))
        .unwrap();

    let actions = expect_apply(&mut rx);
    match &actions[0] {
        Intent::MoveNodes { grabbed, to } => {
            assert_eq!(*grabbed, id);
            assert_eq!(to.len(), 1);
            assert_eq!(to[0].0, id);
            assert_eq!(to[0].1, Vec2::new(5.0, -6.5));
        }
        other => panic!("expected MoveNodes, got {other:?}"),
    }
}

#[test]
fn prelude_select_node_decodes_to_setselection() {
    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    let engine = build_engine(
        state,
        tx,
        Arc::new(ArcSwap::from_pointee(Library::default())),
    );

    let id = NodeId::unique();
    engine
        .eval::<()>(&format!(r#"select_node("{id}")"#))
        .unwrap();

    let actions = expect_apply(&mut rx);
    match &actions[0] {
        Intent::SetSelection { to } => {
            assert_eq!(to.len(), 1);
            assert!(to.contains(&id));
        }
        other => panic!("expected SetSelection, got {other:?}"),
    }
}

#[test]
fn run_emits_run_once() {
    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    let engine = build_engine(
        state,
        tx,
        Arc::new(ArcSwap::from_pointee(Library::default())),
    );

    engine.eval::<()>("run()").unwrap();
    assert!(matches!(rx.try_recv(), Ok(ScriptMessage::RunOnce)));
}

#[test]
fn shutdown_emits_shutdown() {
    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    let engine = build_engine(
        state,
        tx,
        Arc::new(ArcSwap::from_pointee(Library::default())),
    );

    engine.eval::<()>("shutdown()").unwrap();
    assert!(matches!(rx.try_recv(), Ok(ScriptMessage::Shutdown)));
}
