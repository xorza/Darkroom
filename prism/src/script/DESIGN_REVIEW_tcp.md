# Design review: `prism/src/script/tcp.rs`  (2026-04-24)

## Current design

One file, three layers. **Construction** (`TcpTransport::bind`) synchronously binds a `std::net::TcpListener` — so the caller learns the port *before* the accept loop starts — and parks it with a `TcpTimeouts` struct and the optional auth `Uuid`. **Boot** (`tcp::start`) adds the discovery-file write and returns a side-effect-free `TcpStartReport`. **Runtime** (`run_listener` + `handle_conn`) converts the std listener into a tokio one, gates concurrent connections through `Semaphore(MAX_CONNECTIONS)` via `try_acquire_owned`, and runs a per-connection loop that auths once (under `timeouts.auth`) and then loops over `[16B session-id][u32 src-len][src]` frames — each read/write wrapped in `tokio::time::timeout` with a dedicated error message.

Three load-bearing decisions: (a) the listener lives inside the transport so `bind` can report the port before `spawn`; (b) the connection semaphore is non-blocking (`try_acquire_owned`), so the accept loop never stalls — extras are closed before auth runs; (c) the reply type (`ScriptResult`) owns the wire shape via derive-serialize, so tcp.rs just does `serde_json::to_string(&reply)`.

## Overall take

Core shape is right. Timeouts per-operation with named semantics (`auth`, `idle`, `frame`, `write`), semaphore-based connection cap, and serialization pushed to the reply struct are all defensible. Findings below are organizational / cosmetic, not structural.

## Findings

### [F1] Token-file rendering + atomic write live in `tcp.rs` despite being transport-agnostic

- **Category**: Responsibility
- **Impact**: 2/5 — speculative today; pays off the day a second transport lands
- **Effort**: 2/5 — move three fns, update one caller
- **Current**: `render_token_file`, `write_token_file`, and `atomic_write` (tcp.rs:165–192) are pure filesystem + serde code. They happen to live here because `tcp::start` is the only caller, and `TcpStartReport.token_file` surfaces the result.
- **Problem**: the operations have no TCP-specific shape. A hypothetical stdio / websocket / named-pipe transport would either reinvent them or import them from `tcp.rs` — which would read oddly because the import path lies about the code's purpose.
- **Alternative**: extract to `script/discovery.rs`:
  ```rust
  pub fn render(port: u16, token: Option<Uuid>) -> String { ... }
  pub fn write(path: &Path, port: u16, token: Option<Uuid>) -> io::Result<PathBuf> { ... }
  ```
  `TcpStartReport.token_file` stays. `tcp::start` imports `script::discovery::write`. `render_token_file`'s public test remains (now in the new module).
- **Recommendation**: **don't do it now.** Until a second transport exists this is premature abstraction. Revisit on the day transport #2 shows up — the extraction is mechanical then.

### [F2] `handle_conn` has four `timeout(...).await.map_err(...)??` repetitions

- **Category**: Control flow / Readability
- **Impact**: 2/5 — no correctness risk, density cost for a reader
- **Effort**: 1/5 — inline helper
- **Current**: `handle_conn` (tcp.rs:278–363) has four near-identical blocks:
  ```rust
  let got = timeout(timeouts.auth, stream.read_u128())
      .await
      .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "auth read timed out"))??;
  ```
  The `map_err(|_| io::Error::new(io::ErrorKind::TimedOut, ...))??` dance repeats in three places (auth, frame body, write). The `match timeout(...).await` on the session-id read takes a different shape because we want to distinguish idle-timeout from EOF from other errors.
- **Problem**: three of the four sites are structurally identical except for the message. Readers notice the pattern and have to verify it really is identical each time.
- **Alternative**: local helper:
  ```rust
  async fn with_timeout<T, F>(dur: Duration, msg: &'static str, fut: F) -> io::Result<T>
  where F: Future<Output = io::Result<T>>,
  {
      tokio::time::timeout(dur, fut).await
          .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, msg))?
  }
  ```
  Each site collapses to one line: `let got = with_timeout(timeouts.auth, "auth read timed out", stream.read_u128()).await?;`. Messages stay greppable. The fourth site (idle read) keeps its explicit match because it has three distinct return paths.
- **Recommendation**: **do it** next time this function is touched. Not worth a standalone commit.

## Considered and rejected

- **Split `tcp.rs` into `tcp/{listener.rs, handle_conn.rs, discovery.rs}` submodules.** 781 lines is large but cohesive — listener accepts into handler, handler needs timeouts, timeouts come from bind. Splitting adds imports without compressing the concept. Skip until a genuine reuse forces the split (see F1 for the only realistic seam).
- **Package `handle_conn`'s six parameters into a struct.** Every param is distinct and non-optional. A `ConnectionContext` struct would read as two layers (caller constructs struct, handler destructures). Six positional args with obvious types is clearer than a struct you have to cross-reference.
- **Replace `std::io::Error` with a structured `ConnError` enum.** Error variance (bad token, frame too large, timeout) currently rides in `io::Error::kind()` + message text. Structured errors would help if we emitted them to clients, but we don't — clients see socket close. Skip.
- **A test-local `bind_loopback(token, timeouts)` helper.** 10+ test sites repeat `TcpTransport::bind(loopback_ephemeral(), token, TcpTimeouts::default())`. Cosmetic dedup; previously raised and declined.

## Scorecard

| # | Finding | Impact | Effort |
|---|---|---|---|
| F1 | Token-file helpers live in `tcp.rs` despite being transport-agnostic | 2 | 2 |
| F2 | Four `timeout().map_err()??` blocks in `handle_conn` | 2 | 1 |

No sharp findings. The module has been through multiple rounds of refactoring and most genuine smells are resolved. F2 is a one-shot readability fix worth grabbing when the file gets touched anyway; F1 waits until transport #2 exists.
