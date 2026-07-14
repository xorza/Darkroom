use super::*;
use crate::core::script::{ScriptExecutor, ScriptMessage, ScriptResult};
use std::net::Ipv4Addr;

/// Port 0 on the loopback interface — the OS picks a free port.
fn loopback_ephemeral() -> SocketAddr {
    SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)
}

async fn spawn_executor_with_transport(
    transport: TcpTransport,
) -> (
    SocketAddr,
    ScriptExecutor,
    mpsc::UnboundedReceiver<ScriptMessage>,
) {
    let addr = transport.local_addr().unwrap();
    let (action_tx, action_rx) = mpsc::unbounded_channel::<ScriptMessage>();
    let executor = ScriptExecutor::new(
        transport,
        action_tx,
        Arc::new(scenarium::Library::default()),
        Arc::new(|| {}),
    );
    (addr, executor, action_rx)
}

/// Send one script request frame: 16B session-id (nil = new) + u32
/// length + source bytes. Use this instead of raw writes so tests
/// stay in sync with the wire format.
async fn send_request(stream: &mut TcpStream, session_id: Option<Uuid>, source: &[u8]) {
    let id = session_id.map(|u| u.as_u128()).unwrap_or(0);
    stream.write_u128(id).await.unwrap();
    let compressed = lz4_flex::block::compress_prepend_size(source);
    let len = u32::try_from(compressed.len()).unwrap();
    stream.write_u32(len).await.unwrap();
    stream.write_all(&compressed).await.unwrap();
}

async fn read_reply(stream: &mut TcpStream) -> String {
    let len = stream.read_u32().await.unwrap() as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await.unwrap();
    let raw = lz4_flex::block::decompress_size_prepended(&buf).unwrap();
    String::from_utf8(raw).unwrap()
}

fn parse_reply(raw: &str) -> ScriptResult {
    serde_json::from_str(raw).expect("reply is a ScriptResult JSON body")
}

#[tokio::test]
async fn no_auth_accepts_script_and_dual_sinks_print() {
    let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
    let (addr, _executor, mut action_rx) = spawn_executor_with_transport(t).await;

    let mut s = TcpStream::connect(addr).await.unwrap();
    send_request(&mut s, None, b"print(\"hi\")").await;

    let reply = parse_reply(&read_reply(&mut s).await);
    assert_eq!(reply.print, "hi\n");
    // `print(...)` is a statement, so the final expression is Unit.
    assert_eq!(reply.result.as_deref(), Some("()\n"));
    assert_eq!(reply.error, None);

    // Status sink: Session gets a tagged Print action.
    let action = tokio::time::timeout(std::time::Duration::from_secs(2), action_rx.recv())
        .await
        .expect("timed out waiting for ScriptMessage::Print")
        .expect("action channel closed");
    match action {
        ScriptMessage::Print { msg } => {
            assert_eq!(msg, "hi");
        }
        other => panic!("expected Print, got {other:?}"),
    }
}

#[tokio::test]
async fn script_result_is_last_expression() {
    let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
    let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

    let mut s = TcpStream::connect(addr).await.unwrap();
    send_request(&mut s, None, b"40 + 2").await;
    let reply = parse_reply(&read_reply(&mut s).await);
    assert_eq!(reply.print, "");
    assert_eq!(reply.result.as_deref(), Some("42\n"));
    assert_eq!(reply.error, None);
    assert!(reply.session.is_some());
}

#[tokio::test]
async fn script_result_object_map() {
    let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
    let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

    let mut s = TcpStream::connect(addr).await.unwrap();
    send_request(&mut s, None, b"#{ a: 1, b: 2 }").await;
    let reply = parse_reply(&read_reply(&mut s).await);
    let r = reply.result.expect("result present");
    // serde_rhai orders map keys alphabetically via BTreeMap iteration.
    assert!(r.starts_with("#{\n"), "got: {r}");
    assert!(r.contains("a: 1,"));
    assert!(r.contains("b: 2,"));
}

#[tokio::test]
async fn session_resume_preserves_scope_across_frames() {
    let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
    let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;
    let mut s = TcpStream::connect(addr).await.unwrap();

    // Frame 1: create session, bind x=42.
    send_request(&mut s, None, b"let x = 42;").await;
    let reply1 = parse_reply(&read_reply(&mut s).await);
    let session = reply1.session.expect("session id returned");

    // Frame 2 on the same connection: resume, read x.
    send_request(&mut s, Some(session), b"x").await;
    let reply2 = parse_reply(&read_reply(&mut s).await);
    assert_eq!(reply2.session, Some(session));
    assert_eq!(reply2.result.as_deref(), Some("42\n"));
}

#[tokio::test]
async fn unknown_session_id_returns_error_and_echoes_id() {
    let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
    let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;
    let mut s = TcpStream::connect(addr).await.unwrap();

    let ghost = Uuid::new_v4();
    send_request(&mut s, Some(ghost), b"1").await;
    let reply = parse_reply(&read_reply(&mut s).await);
    assert_eq!(reply.session, Some(ghost));
    assert!(reply.result.is_none());
    let err = reply.error.expect("error set");
    assert!(err.contains("unknown session"), "got: {err}");
}

#[tokio::test]
async fn script_error_populates_error_field() {
    let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
    let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

    let mut s = TcpStream::connect(addr).await.unwrap();
    // Parse error — Rhai rejects bare `let` without an identifier.
    send_request(&mut s, None, b"let = 1").await;
    let reply = parse_reply(&read_reply(&mut s).await);
    assert_eq!(reply.print, "");
    assert!(reply.result.is_none(), "result should be null on error");
    assert!(reply.error.is_some(), "error should be populated");
}

#[tokio::test]
async fn auth_accepts_correct_token() {
    let token = Uuid::new_v4();
    let t = TcpTransport::bind(loopback_ephemeral(), Some(token), TcpTimeouts::default()).unwrap();
    let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_u128(token.as_u128()).await.unwrap();
    send_request(&mut s, None, b"let x = 1;").await;
    let reply = parse_reply(&read_reply(&mut s).await);
    assert!(reply.error.is_none(), "got error: {:?}", reply.error);
}

#[tokio::test]
async fn auth_rejects_wrong_token() {
    let correct = Uuid::new_v4();
    let wrong = Uuid::new_v4();
    let t =
        TcpTransport::bind(loopback_ephemeral(), Some(correct), TcpTimeouts::default()).unwrap();
    let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_u128(wrong.as_u128()).await.unwrap();
    // Server sees the bad token and drops the stream cleanly (FIN
    // with no bytes written), so the client's first read returns
    // EOF — `read` returns Ok(0), `read_u32` maps that to
    // UnexpectedEof. Pin both so a future change that accidentally
    // sends a partial reply on reject would fail this test.
    let mut buf = [0u8; 1];
    assert_eq!(s.read(&mut buf).await.unwrap(), 0, "expected clean EOF");
    let err = s.read_u32().await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
}

/// A `u32` length above `MAX_FRAME_BYTES` is rejected before any
/// allocation. Server drops the connection without a reply.
#[tokio::test]
async fn oversized_frame_is_rejected() {
    let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
    let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

    let mut s = TcpStream::connect(addr).await.unwrap();
    // Nil session id, then an oversized length declaration.
    s.write_u128(0).await.unwrap();
    s.write_u32(MAX_FRAME_BYTES + 1).await.unwrap();
    // Server bails from `read_string_frame` with InvalidData and
    // drops the stream; we should see EOF, not a JSON reply.
    let mut buf = [0u8; 1];
    assert_eq!(s.read(&mut buf).await.unwrap(), 0, "expected EOF on reject");
}

#[test]
fn render_token_file_with_token() {
    let token = Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
    let s = render_token_file(45678, Some(token));
    // Serde's pretty printer uses exact indent/newline layout; this
    // assertion pins the on-disk shape so clients can parse stably.
    assert_eq!(
        s,
        "{\n  \"port\": 45678,\n  \"token\": \"12345678-9abc-def0-1122-334455667788\"\n}"
    );
}

#[test]
fn render_token_file_without_token() {
    let s = render_token_file(1234, None);
    assert_eq!(s, "{\n  \"port\": 1234,\n  \"token\": null\n}");
}

#[tokio::test]
async fn start_reports_bound_addr_and_token() {
    let token = Uuid::new_v4();
    let cfg = TcpScriptConfig {
        bind: loopback_ephemeral(),
        token: Some(token),
        token_file: None,
    };
    let (transport, report) = start(&cfg).unwrap();
    // Bound addr matches the transport's own local_addr.
    assert_eq!(report.addr, transport.local_addr().unwrap());
    assert_eq!(report.addr.port(), transport.local_addr().unwrap().port());
    assert_eq!(report.token, Some(token));
    assert!(report.token_file.is_none());
}

#[tokio::test]
async fn start_writes_discovery_file_atomically() {
    use std::fs;

    // Per-test directory under the system temp root; Uuid keeps
    // parallel test runs from colliding. Clean up at the end.
    let dir = std::env::temp_dir().join(format!("darkroom-test-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("discovery.json");

    let token = Uuid::from_u128(42);
    let cfg = TcpScriptConfig {
        bind: loopback_ephemeral(),
        token: Some(token),
        token_file: Some(path.clone()),
    };
    let (_transport, report) = start(&cfg).unwrap();

    assert_eq!(
        report.token_file.as_ref().unwrap().as_deref(),
        Ok(path.as_path())
    );

    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains(&format!("\"port\": {}", report.addr.port())));
    assert!(body.contains(&token.to_string()));

    // Temp file must not linger beside the real one.
    let tmp = path.with_file_name("discovery.json.tmp");
    assert!(!tmp.exists(), "tmp file should have been renamed");

    let _ = fs::remove_dir_all(&dir);
}

// ----- Timeout tests -----
//
// Every timeout test sets *only* the field it probes to a short
// value; the other three stay long (10s) so the test pins down
// which timeout actually fired. If `handle_conn` accidentally
// wired the wrong field to a read point, the intended test fails
// instead of passing for the wrong reason.

const TEST_LONG: Duration = Duration::from_secs(10);
const TEST_SHORT: Duration = Duration::from_millis(150);

fn long_timeouts() -> TcpTimeouts {
    TcpTimeouts {
        auth: TEST_LONG,
        idle: TEST_LONG,
        frame: TEST_LONG,
        write: TEST_LONG,
    }
}

/// Quiet client that never sends a frame is closed by the
/// between-frames idle timeout specifically.
#[tokio::test]
async fn idle_connection_closes_after_timeout() {
    let timeouts = TcpTimeouts {
        idle: TEST_SHORT,
        ..long_timeouts()
    };
    let t = TcpTransport::bind(loopback_ephemeral(), None, timeouts).unwrap();
    let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

    let mut s = TcpStream::connect(addr).await.unwrap();
    let start = std::time::Instant::now();
    let mut buf = [0u8; 4];
    let n = s.read(&mut buf).await.unwrap_or(0);
    assert_eq!(n, 0, "expected EOF, got {n} bytes");
    // Close must come from `idle` (150ms), not the other 10s fields.
    assert!(
        start.elapsed() >= Duration::from_millis(100),
        "closed too fast: {:?}",
        start.elapsed()
    );
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "closed too slow — probably tripped a different timeout: {:?}",
        start.elapsed()
    );
}

/// With auth on, a client that connects but never sends the token
/// is closed by the auth timeout specifically.
#[tokio::test]
async fn auth_timeout_closes_silent_client() {
    let timeouts = TcpTimeouts {
        auth: TEST_SHORT,
        ..long_timeouts()
    };
    let token = Uuid::new_v4();
    let t = TcpTransport::bind(loopback_ephemeral(), Some(token), timeouts).unwrap();
    let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

    let mut s = TcpStream::connect(addr).await.unwrap();
    let start = std::time::Instant::now();
    let mut buf = [0u8; 4];
    let n = s.read(&mut buf).await.unwrap_or(0);
    assert_eq!(n, 0);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "closed too slow — probably tripped a different timeout: {:?}",
        start.elapsed()
    );
}

/// A client that sends the session-id prefix and stalls is closed
/// by the mid-frame timeout specifically.
#[tokio::test]
async fn partial_frame_closes_after_frame_timeout() {
    let timeouts = TcpTimeouts {
        frame: TEST_SHORT,
        ..long_timeouts()
    };
    let t = TcpTransport::bind(loopback_ephemeral(), None, timeouts).unwrap();
    let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

    let mut s = TcpStream::connect(addr).await.unwrap();
    // Send 16B nil session id, then stall (no length-prefix, no src).
    s.write_u128(0).await.unwrap();
    let start = std::time::Instant::now();
    let mut buf = [0u8; 4];
    let n = s.read(&mut buf).await.unwrap_or(0);
    assert_eq!(n, 0);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "closed too slow — probably tripped a different timeout: {:?}",
        start.elapsed()
    );
}

/// Beyond MAX_CONNECTIONS concurrent clients, extras are closed at
/// accept time without going through auth or the executor.
#[tokio::test]
async fn excess_connections_are_refused() {
    // Default timeouts so the holders don't get reaped mid-test.
    let t = TcpTransport::bind(loopback_ephemeral(), None, TcpTimeouts::default()).unwrap();
    let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

    // Hold MAX_CONNECTIONS sockets open. For each, send+receive a
    // no-op script: the reply proves the server has already
    // accepted, authed, and taken the permit. No wall-clock sleep.
    let mut holders = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        let mut s = TcpStream::connect(addr).await.unwrap();
        send_request(&mut s, None, b"1").await;
        let reply = parse_reply(&read_reply(&mut s).await);
        assert_eq!(reply.error, None);
        holders.push(s);
    }

    // Over-the-limit connect: accept() succeeds (OS-level 3-way
    // handshake), but the server drops the stream immediately, so
    // our read sees EOF.
    let mut extra = TcpStream::connect(addr).await.unwrap();
    let mut buf = [0u8; 4];
    let n = extra.read(&mut buf).await.unwrap_or(0);
    assert_eq!(n, 0, "over-limit connection should be closed immediately");

    // Drop one holder → its permit frees. We need the server's spawned
    // task to observe the close and release the permit, so probe with
    // a retry loop instead of a fixed sleep.
    drop(holders.pop());
    let mut admitted = None;
    for _ in 0..50 {
        let mut s = TcpStream::connect(addr).await.unwrap();
        send_request(&mut s, None, b"1").await;
        let mut hdr = [0u8; 4];
        if s.peek(&mut hdr).await.unwrap_or(0) == 4 {
            admitted = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut admitted = admitted.expect("permit should free after holder closes");
    let reply = parse_reply(&read_reply(&mut admitted).await);
    assert_eq!(reply.error, None);
}
