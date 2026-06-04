use super::*;
use scenarium::graph::{Binding, NodeKind, OutputPort};
use std::net::Ipv4Addr;

/// Build an `InboundSender` paired with the receiver tests assert on.
/// `notify` is a no-op — tests don't drive a real host loop.
fn test_inbound() -> (InboundSender, mpsc::UnboundedReceiver<SessionInbound>) {
    let (tx, rx) = mpsc::unbounded_channel::<SessionInbound>();
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
fn expect_apply(rx: &mut mpsc::UnboundedReceiver<SessionInbound>) -> Vec<Intent> {
    match rx.try_recv().expect("Apply queued") {
        SessionInbound::Apply(actions) => actions,
        other => panic!("expected Apply, got {other:?}"),
    }
}

#[test]
fn list_funcs_returns_full_func_objects_in_insertion_order() {
    use scenarium::function::{Func, FuncId};

    let mut lib = FuncLib::default();
    lib.add(Func {
        id: FuncId::unique(),
        name: "alpha".to_string(),
        category: "math".to_string(),
        ..Default::default()
    });
    lib.add(Func {
        id: FuncId::unique(),
        name: "beta".to_string(),
        category: "io".to_string(),
        ..Default::default()
    });

    let state = Arc::new(Mutex::new(String::new()));
    let (tx, _rx) = test_inbound();
    let engine = build_engine(state, tx, Arc::new(lib));

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
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

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
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

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
    // Empty FuncLib → any well-formed UUID is "unknown".
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

    let err = engine
        .eval::<String>(r#"create_node("00000000-0000-0000-0000-000000000001", 0.0, 0.0)"#)
        .expect_err("unknown id should error");
    assert!(err.to_string().contains("unknown func id"), "got: {err}");
    assert!(rx.try_recv().is_err());
}

#[test]
fn create_node_known_id_enqueues_add_node() {
    use scenarium::function::{Func, FuncId};

    let alpha_id = FuncId::unique();
    let mut lib = FuncLib::default();
    lib.add(Func {
        id: alpha_id,
        name: "alpha".to_string(),
        ..Default::default()
    });

    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    let engine = build_engine(state, tx, Arc::new(lib));

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
        } => {
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
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

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
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

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
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

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
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

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
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

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
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

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
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

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
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

    engine.eval::<()>("run()").unwrap();
    assert!(matches!(rx.try_recv(), Ok(SessionInbound::RunOnce)));
}

#[test]
fn shutdown_emits_shutdown() {
    let state = Arc::new(Mutex::new(String::new()));
    let (tx, mut rx) = test_inbound();
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

    engine.eval::<()>("shutdown()").unwrap();
    assert!(matches!(rx.try_recv(), Ok(SessionInbound::Shutdown)));
}

#[test]
fn build_transports_empty_when_no_tcp_config() {
    // Guards against accidentally re-enabling an always-on listener.
    let cfg = ScriptConfig::default();
    assert!(cfg.tcp.is_none());
    let results = build_transports(&cfg);
    assert!(results.is_empty());
}

#[test]
fn build_transports_returns_started_tcp_with_report() {
    let token = Uuid::new_v4();
    let cfg = ScriptConfig {
        tcp: Some(TcpScriptConfig {
            bind: SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0),
            token: Some(token),
            token_file: None,
        }),
    };
    let mut results = build_transports(&cfg);
    assert_eq!(results.len(), 1);
    let started = results
        .remove(0)
        .expect("bind should succeed on loopback :0");
    let TransportReport::Tcp(report) = &started.report;
    assert_eq!(report.token, Some(token));
    assert!(report.addr.ip().is_loopback());
    assert_ne!(report.addr.port(), 0, "OS should have assigned a real port");
    assert!(report.token_file.is_none());
}

#[test]
fn build_transports_surfaces_bind_failure() {
    // Bind once to pin the port, then ask for the same port again so the
    // second bind fails. Loopback :0 lets the OS pick; we read the port
    // off the first listener and reuse it.
    let first = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let taken = first.local_addr().unwrap();

    let cfg = ScriptConfig {
        tcp: Some(TcpScriptConfig {
            bind: taken,
            token: None,
            token_file: None,
        }),
    };
    let mut results = build_transports(&cfg);
    assert_eq!(results.len(), 1);
    let err = results
        .remove(0)
        .expect_err("port already taken should fail");
    assert_eq!(err.kind, TransportKind::Tcp);
    // AddrInUse on Linux/mac; pin the kind so a regression that silently
    // swallows the error (e.g. None on bind failure) trips.
    assert_eq!(err.error.kind(), std::io::ErrorKind::AddrInUse);
}

#[test]
fn parse_bind_spec_variants() {
    // Bare port (with and without the leading colon) → loopback.
    assert_eq!(
        parse_bind_spec("34567").unwrap(),
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 34567)
    );
    assert_eq!(
        parse_bind_spec(":8080").unwrap(),
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 8080)
    );
    // Bare IP → default port (and "::1" must not be mangled by the
    // colon-strip).
    assert_eq!(
        parse_bind_spec("0.0.0.0").unwrap(),
        SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), DEFAULT_SCRIPT_PORT)
    );
    assert_eq!(
        parse_bind_spec("::1").unwrap(),
        SocketAddr::new(std::net::Ipv6Addr::LOCALHOST.into(), DEFAULT_SCRIPT_PORT)
    );
    // Full socket addr passes through.
    assert_eq!(
        parse_bind_spec("127.0.0.1:9000").unwrap(),
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 9000)
    );
    assert!(parse_bind_spec("not-an-addr").is_err());
}

#[test]
fn script_args_default_disables_listener() {
    assert!(ScriptArgs::default().to_config().tcp.is_none());
}

#[test]
fn script_args_tcp_mints_token_and_uses_default_bind() {
    let cfg = ScriptArgs {
        script_tcp: true,
        ..Default::default()
    }
    .to_config();
    let tcp = cfg.tcp.expect("listener enabled");
    assert!(tcp.token.is_some(), "auth on by default");
    assert_eq!(tcp.bind, default_bind());
}

#[test]
fn script_args_no_auth_clears_token() {
    let cfg = ScriptArgs {
        script_tcp: true,
        script_no_auth: true,
        ..Default::default()
    }
    .to_config();
    assert!(cfg.tcp.unwrap().token.is_none());
}

#[test]
fn script_args_explicit_token_and_bind() {
    let token = Uuid::new_v4();
    let cfg = ScriptArgs {
        script_tcp: true,
        script_bind: Some(":9999".into()),
        script_token: Some(token),
        ..Default::default()
    }
    .to_config();
    let tcp = cfg.tcp.unwrap();
    assert_eq!(tcp.token, Some(token));
    assert_eq!(tcp.bind.port(), 9999);
}
