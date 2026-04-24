# Design review: `prism/src/script/tcp.rs`  (2026-04-24)

## Current design

Three layers in one file: construction (`TcpTransport::bind`) binds a sync `std::net::TcpListener` so the caller learns the port before the accept loop starts; boot (`tcp::start`) adds discovery-file writing and returns a pure `TcpStartReport` for the caller to surface; runtime (`run_listener` + `handle_conn`) converts the std listener into a tokio one, gates concurrent connections through a `Semaphore(MAX_CONNECTIONS)`, and runs a per-connection loop that auths once, then reads `[16B session-id][u32 src-len][src]` frames, dispatches to the executor, and writes `[u32 len][JSON]` replies.

Every I/O wait is wrapped in `tokio::time::timeout`: `auth` (10s), `idle` between frames (10min), `frame` mid-frame (30s), `write` (30s). Timeouts are `Copy`-captured; no Arc. Connection permits are `try_acquire_owned` (non-blocking) so the accept loop never stalls on a full semaphore; permits drop with the handler task via RAII.

## Overall take

Core shape is right. Synchronous bind before the accept loop, per-operation timeouts with named semantics, semaphore for concurrency cap, derive-serialized reply — all defensible choices. Findings below are test-suite weaknesses and small polish. No structural rethink.

## Findings

### [F1] `short_timeouts()` sets every field to 150ms; timeout-specific tests don't prove which timeout fires

- **Category**: Contract / Test fidelity
- **Impact**: 3/5 — tests pass even if `handle_conn` used the wrong timeout for a given read point
- **Effort**: 1/5 — one helper + per-test overrides
- **Current**: `short_timeouts` (tcp.rs:652–659) returns `{auth: 150ms, idle: 150ms, frame: 150ms, write: 150ms}`. Tests `auth_timeout_closes_silent_client` (tcp.rs:689), `idle_connection_closes_after_timeout` (tcp.rs:664), `partial_frame_closes_after_frame_timeout` (tcp.rs:704) all use it. Each test closes within ~150ms, so each passes — but none of them would notice if `handle_conn` swapped `timeouts.idle` for `timeouts.frame` at the session-id read, or applied `timeouts.idle` to the auth read instead of `timeouts.auth`.
- **Problem**: the tests claim to probe *which* timeout fires, but all four are equal so they only prove "some timeout fires under 150ms." A refactor that collapses two timeout sites into one would silently continue to pass.
- **Alternative**: make the other three fields long in `short_timeouts`, and let each test tighten the *one* timeout it probes:
  ```rust
  fn short_timeouts() -> TcpTimeouts {
      let long = Duration::from_secs(10);
      TcpTimeouts { auth: long, idle: long, frame: long, write: long }
  }
  // In auth_timeout_closes_silent_client:
  let t = TcpTransport::bind(addr, Some(token), TcpTimeouts {
      auth: Duration::from_millis(150),
      ..short_timeouts()
  }).unwrap();
  ```
  Now each test proves its intended timeout is the one being hit — any misrouted `timeouts.auth`→`timeouts.idle` swap in production code fails the relevant test.
- **Recommendation**: do it.

### [F2] `send_raw_len_bytes` is a one-use helper that obscures what `auth_rejects_wrong_token` is testing

- **Category**: Abstraction / Test readability
- **Impact**: 2/5 — deletable helper, clearer test
- **Effort**: 1/5
- **Current**: `send_raw_len_bytes` (tcp.rs:422–427) is written specifically because `auth_rejects_wrong_token` (tcp.rs:558) sends a payload that the server is *already going to ignore* — the server closed the stream on the token mismatch before this write arrives. The test comment (lines 568–571) explains, but the helper's name ("legacy frame-writer") hints at a now-absent reason.
- **Problem**: the extra write is defensive but conceptually irrelevant to the assertion. A reader needs the comment *and* the helper *and* the test to figure out what's actually being proved (EOF on read).
- **Alternative**: drop the extra write and the helper. The test becomes:
  ```rust
  #[tokio::test]
  async fn auth_rejects_wrong_token() {
      let correct = Uuid::new_v4();
      let wrong = Uuid::new_v4();
      let t = TcpTransport::bind(loopback_ephemeral(), Some(correct), TcpTimeouts::default()).unwrap();
      let (addr, _executor, _rx) = spawn_executor_with_transport(t).await;

      let mut s = TcpStream::connect(addr).await.unwrap();
      s.write_u128(wrong.as_u128()).await.unwrap();
      // Server sees bad token, drops the stream. Our read sees EOF.
      assert!(s.read_u32().await.is_err());
  }
  ```
  Four lines shorter, no helper, same assertion. If we later want to prove the server ignores subsequent writes specifically (it does by virtue of the socket being closed), that's a different test.
- **Recommendation**: do it.

### [F3] `Ok(Ok(0)) => None` in the idle-read match is an unannotated magic literal

- **Category**: Types / Readability
- **Impact**: 1/5 — cosmetic
- **Effort**: 1/5
- **Current**: In `handle_conn` (tcp.rs:314), the `match timeout(...).await` has a separate arm for `Ok(Ok(0))` that means "client asked for a new session." The `0` is `Uuid::nil().as_u128()`, but the connection between them is only in the module-level doc comment.
- **Alternative (a)**: `const NIL_SESSION_ID: u128 = 0;` with a doc-comment pointing at `Uuid::nil()`. Replace `Ok(Ok(0))` with `Ok(Ok(NIL_SESSION_ID))`.
- **Alternative (b)**: collapse the two `Ok(Ok(_))` arms by reading the u128 first, then mapping to `Option<Uuid>` in a separate statement:
  ```rust
  let raw = match timeout(timeouts.idle, stream.read_u128()).await {
      Err(_) => { ... return Ok(()); }
      Ok(Err(e)) if e.kind() == UnexpectedEof => return Ok(()),
      Ok(Err(e)) => return Err(e),
      Ok(Ok(v)) => v,
  };
  let session_id = (raw != 0).then(|| Uuid::from_u128(raw));
  ```
- **Recommendation**: (b) — reduces the 5-arm match to 4 and names the intent.

## Considered and rejected

- **`TcpTransport::bind` taking three required args is clunky; revert to two constructors.** User explicitly merged earlier. Tests paying the ergonomic cost is acceptable; production has one callsite. A builder is over-engineering for three fields.
- **Add a reply-size cap.** `MAX_FRAME_BYTES = 1 MiB` only gates inbound; the reply can be much larger. But Rhai's `MAX_STRING_SIZE=1 MiB × MAX_MAP_LEN=100k` already bounds the Dynamic's serialized form far below anything that would matter. Speculative.
- **Apply timeout to `listener.accept()`.** `accept()` only "stalls" when no clients are connecting — that's by definition. No timeout here.
- **Hold the accept loop when the semaphore is full (blocking `acquire` instead of `try_acquire_owned`).** Would serialize new-connection probes behind a permit, delaying the reject path — worse behavior. Current shape (accept, reject fast) is right.

## Scorecard

| # | Finding | Impact | Effort |
|---|---|---|---|
| F1 | `short_timeouts()` makes timeout-probing tests non-discriminating | 3 | 1 |
| F2 | `send_raw_len_bytes` helper is dead weight in `auth_rejects_wrong_token` | 2 | 1 |
| F3 | `Ok(Ok(0))` in idle-read match is unannotated magic | 1 | 1 |

F1 is the one with real teeth — the current test suite doesn't prove the timeout routing is correct. Apply all three in one pass; all effort=1.
