use super::*;
use std::net::Ipv4Addr;

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

    let state = Arc::new(Mutex::new(RequestState::default()));
    let (tx, _rx) = mpsc::unbounded_channel::<ScriptAction>();
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
fn create_node_malformed_id_returns_rhai_error_and_no_action() {
    let state = Arc::new(Mutex::new(RequestState::default()));
    let (tx, mut rx) = mpsc::unbounded_channel::<ScriptAction>();
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

    let err = engine
        .eval::<()>(r#"create_node("not-a-uuid", 0.0, 0.0)"#)
        .expect_err("malformed id should error");
    assert!(err.to_string().contains("invalid func id"), "got: {err}");
    assert!(rx.try_recv().is_err());
}

#[test]
fn create_node_unknown_id_returns_rhai_error_and_no_action() {
    let state = Arc::new(Mutex::new(RequestState::default()));
    let (tx, mut rx) = mpsc::unbounded_channel::<ScriptAction>();
    // Empty FuncLib → any well-formed UUID is "unknown".
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

    let err = engine
        .eval::<()>(r#"create_node("00000000-0000-0000-0000-000000000001", 0.0, 0.0)"#)
        .expect_err("unknown id should error");
    assert!(err.to_string().contains("unknown func id"), "got: {err}");
    assert!(rx.try_recv().is_err());
}

#[test]
fn create_node_known_id_enqueues_node_added_action() {
    use scenarium::function::{Func, FuncId};

    let alpha_id = FuncId::unique();
    let mut lib = FuncLib::default();
    lib.add(Func {
        id: alpha_id,
        name: "alpha".to_string(),
        ..Default::default()
    });

    let state = Arc::new(Mutex::new(RequestState::default()));
    let (tx, mut rx) = mpsc::unbounded_channel::<ScriptAction>();
    let engine = build_engine(state, tx, Arc::new(lib));

    let script = format!(r#"create_node("{alpha_id}", 12.5, -3.0)"#);
    engine.eval::<()>(&script).unwrap();

    // The executor builds a fully-formed GraphUiAction::NodeAdded —
    // identical to what the GUI emits — and ships it via Apply. Session
    // applies it through the same commit path with no script-aware glue.
    let action = rx.try_recv().expect("Apply action queued");
    match action {
        ScriptAction::Apply(GraphUiAction::NodeAdded { view_node, node }) => {
            assert_eq!(node.func_id, alpha_id);
            assert_eq!(node.name, "alpha");
            assert_eq!(view_node.id, node.id);
            assert_eq!(view_node.pos, egui::Pos2::new(12.5, -3.0));
        }
        other => panic!("expected Apply(NodeAdded), got {other:?}"),
    }
}

#[test]
fn list_funcs_is_empty_when_func_lib_is_empty() {
    let state = Arc::new(Mutex::new(RequestState::default()));
    let (tx, _rx) = mpsc::unbounded_channel::<ScriptAction>();
    let engine = build_engine(state, tx, Arc::new(FuncLib::default()));

    let result: Array = engine.eval("list_funcs()").unwrap();
    assert!(result.is_empty());
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
    // Bind once to pin the port, then ask for the same port again so
    // the second bind fails. Loopback :0 lets the OS pick; we read
    // the port off the first listener and reuse it.
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
    // AddrInUse on Linux/mac; pin the kind so a regression that
    // silently swallows the error (e.g. None on bind failure) trips.
    assert_eq!(err.error.kind(), std::io::ErrorKind::AddrInUse);
}
