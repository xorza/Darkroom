# Design review: `app_config.rs` + `script/`  (2026-04-24)

## Current design

**`app_config.rs`** owns CLI-facing types: the clap-flattened `ScriptCliArgs`, the `DEFAULT_SCRIPT_PORT = 33433` constant, a flexible `parse_bind_spec` accepting bare port / bare IP / full `addr:port`, and a pure `build_script_config(args, fresh_token) -> ScriptConfig` whose `fresh_token` fallback lets `main` inject `Uuid::new_v4()` while tests inject a fixed value. It also holds a one-field wrapper `AppConfig { script: ScriptConfig }` that every frontend constructor takes and immediately unwraps.

**`script/mod.rs`** defines the scripting surface: `ScriptConfig { tcp: Option<TcpScriptConfig> }`, the `ScriptTransport` trait (one `start(self: Box<Self>, tx, cancel) -> TransportHandle` method), `ScriptExecutor` that owns a background tokio task and a vec of transport handles, and the shared message types (`ScriptRequest`, `ScriptResult`, `ScriptAction`). `build_transports` eagerly binds each enabled transport so the caller learns OS-assigned ports synchronously; the companion `announce_tcp` writes the banner to stdout. Rhai engine caps are module-private constants.

**`script/tcp.rs`** implements the one real transport. `TcpTransport::bind` does a sync `std::net::TcpListener::bind` so the bound port is visible before the accept loop starts; `tcp::start(cfg)` composes bind + optional discovery-file write and returns `(TcpTransport, TcpStartReport)` — a pure summary record for the caller to surface. Auth is a 16-byte UUID prefix (branchless u128 XOR compare), framing is `[u32-be len][utf8 source]`, one script per connection.

## Overall take

The layering is sound: pure config builder → eager bind → async accept loop → shared executor. The recent refactor pass (side-effect-free `start`, split `render_token_file`, `ScriptStartReport`, `ScriptCliArgs` as a pure struct) removed the worst testability blockers. Remaining findings are shape nits and one genuine protocol inconsistency.

## Findings

### [F1] `ScriptResult.stdout` is a dead field; protocol contract doesn't match the code

- **Category**: Contract / State that shouldn't exist
- **Impact**: 4/5 — documented protocol says the reply frame carries stdout, the code always makes it empty
- **Effort**: 2/5 — local to `script/mod.rs` and `tcp::handle_conn`
- **Current**: `run_script` (script/mod.rs:297) unconditionally sets `stdout: String::new()` on both branches. Rhai `print` output is instead routed to `ScriptAction::Print` via the `action_tx` channel (line 280–282). `handle_conn` (tcp.rs:230–233) formats the reply as `format!("ERROR: {err}\n{}", reply.stdout)` — on success the TCP client receives a zero-length frame. The skill's protocol doc tells clients the reply is "captured stdout."
- **Problem**: two senders (the TCP protocol doc and the action-channel wiring) make contradictory claims about where `print` output goes. `stdout: String` is a ghost field that three readers interact with but nobody fills. A TCP client running `print("hi")` today gets back `""`, not `"hi"`.
- **Alternative A (trim)**: drop `stdout` from `ScriptResult`. Reply becomes `""` on success, `format!("ERROR: {msg}")` on error. Update protocol doc to say "reply is empty on success; errors are prefixed `ERROR: `."
- **Alternative B (fix)**: capture `print` output into a per-request buffer (thread-local or closure-captured `Vec<String>`) and include it in the reply. Keeps the existing protocol promise. This is real work — `engine.on_print` is global, not per-request.
- **Recommendation**: pick one and commit. Given the action-channel already carries print output into Session's status bar (the intended user-visible path), A is the path of least surprise. If any out-of-process client needs to see print output, B is the right answer.

### [F2] `TcpStartReport.token_file_written` + `token_file_error` encodes an invariant in a comment

- **Category**: Types / Contract
- **Impact**: 3/5 — removes a load-bearing comment and an unrepresentable state
- **Effort**: 1/5 — local to `tcp.rs` and `announce_tcp`
- **Current**: `TcpStartReport` (tcp.rs:55–65) has two `Option<_>` fields guarded by a comment: "Independent of `token_file_written` — never both `Some`." Readers (announce_tcp at script/mod.rs:88–92, tests) branch on `error.is_some()` / `written.is_some()`.
- **Problem**: the "never both `Some`" invariant is English, not Rust. A future field write (`report.token_file_error = Some(...)` on the happy path by mistake) compiles and runs silently.
- **Alternative**: collapse into `token_file: Option<Result<PathBuf, String>>`. `None` = not requested. `Some(Ok(path))` = written. `Some(Err(msg))` = requested but failed. The four cases become three, enforced by the type.
- **Recommendation**: do it.

### [F3] `AppConfig` is a one-field wrapper with no second field on the horizon

- **Category**: Premature generalization
- **Impact**: 3/5 — removes a type, five imports, and a `.script` indirection per frontend
- **Effort**: 1/5 — mechanical rename across 5 sites
- **Current**: `AppConfig { script: ScriptConfig }` (app_config.rs:20–23) is created in main.rs:42, threaded through `gui::run` (gui/mod.rs:40), `tui::run` (tui/mod.rs:16), `GuiApp::new` (gui/app.rs:16), `TuiApp::new` (tui/app.rs:15), and each frontend immediately destructures it into `app_config.script`. No planned second field — the project has no general config-file plans from CLAUDE.md.
- **Problem**: "future-proofing for more knobs" was rationalized up front; the knob never arrived. Today it's one extra type, one extra import per caller, one extra `.script` access per construction.
- **Alternative**: frontends take `ScriptConfig` directly. When the second runtime knob arrives, promote to an aggregate at that point — rename + wrap, three minutes of work.
- **Recommendation**: do it. Inverting premature abstractions is cheapest while they're fresh.

### [F4] `ScriptTransport` trait has exactly one implementor

- **Category**: Premature generalization / Abstraction
- **Impact**: 3/5 — deletes a trait, removes `Box<dyn>` dispatch, simplifies `ScriptExecutor::new`'s generic signature
- **Effort**: 2/5 — touches trait def, `impl` block, executor signature, `build_transports` return type
- **Current**: `trait ScriptTransport: Send` (script/mod.rs:191–197) has one method and one impl (`TcpTransport` at tcp.rs:131–147). `ScriptExecutor::new` takes `I: IntoIterator<Item = Box<dyn ScriptTransport>>` (line 213–215); `build_transports` returns `Vec<Box<dyn ScriptTransport>>` populated from a single enabled variant.
- **Problem**: abstraction with no second concrete type is speculative. The `start` method's signature is TCP-shaped (channel-to-executor + cancellation token) — it doesn't already support a meaningfully different transport shape. Future transports would likely require trait changes anyway.
- **Alternative A**: drop the trait entirely. `ScriptExecutor::new(tcp_transports: Vec<TcpTransport>, action_tx)`. When stdio/websocket arrives, either re-introduce the trait (if shapes genuinely differ) or use an enum `Transport::Tcp(TcpTransport) | Transport::Stdio(...)`.
- **Alternative B**: keep the trait but drop `Box<dyn>` by making `ScriptExecutor` generic over a concrete transport set (`ScriptExecutor<T: ScriptTransport>`). Doesn't help when there are multiple concrete transports per app; unlikely to be net simpler.
- **Recommendation**: **depends on planned transports.** If a second transport is within a month, keep the trait to avoid thrashing. If the roadmap is TCP-only for the foreseeable future, collapse. The `ScriptTransport` name + `start(Box<Self>, ...)` surface is cheap to reintroduce later.

### [F5] `DEFAULT_SCRIPT_PORT` drifts silently from `default_value = "127.0.0.1:33433"`

- **Category**: Contract / Data
- **Impact**: 2/5 — prevents a quiet drift when someone edits the const
- **Effort**: 1/5 — one-line attribute swap
- **Current**: `pub const DEFAULT_SCRIPT_PORT: u16 = 33433` (app_config.rs:18) is the documented source of truth, but the clap default on `script_bind` uses the string literal `"127.0.0.1:33433"` (app_config.rs:44) which does not reference the const. The `default_bind()` helper (line 92) also uses the const but isn't wired to clap.
- **Problem**: if `DEFAULT_SCRIPT_PORT` changes to, say, 40000, the code compiles; the clap default stays 33433. Silent divergence between "the docs say 40000" and "the CLI says 40000 but the default is still 33433."
- **Alternative**: `default_value_t = default_bind()` instead of `default_value = "..."`. `SocketAddr` implements `Display` and `Clone`; clap supports `default_value_t`.
- **Recommendation**: do it.

## Considered and rejected

- **Collapse `ScriptConfig`'s `tcp: Option<_>` into an enum** (`Disabled | Enabled(TcpScriptConfig)`). Marginal clarity win today; bakes in "one transport" when the natural direction is multi-transport records. `Option` is right.
- **Drop `--script-token-file` now that `--script-token` exists.** Different ops scenarios prefer different channels (subprocess capture of stdout, systemd unit reading a file, client already knows its UUID). Redundancy is harmless; each path has a legitimate user.
- **Empty-Vec → no-executor skip** (`Session::script_executor = None` when `build_transports` returns an empty vec). Saves one idle tokio task. Too small to matter; `ScriptExecutor` is opaque about this trade today and that's fine.

## Scorecard

| # | Finding | Impact | Effort |
|---|---|---|---|
| F1 | ScriptResult.stdout is dead / protocol mismatch | 4 | 2 |
| F2 | TcpStartReport split Option pair → Option<Result> | 3 | 1 |
| F3 | AppConfig one-field wrapper | 3 | 1 |
| F4 | ScriptTransport trait with one impl | 3 | 2 |
| F5 | DEFAULT_SCRIPT_PORT drifts from clap default | 2 | 1 |

Do F2, F3, F5 together in one pass (all Effort 1, 8/15 total impact). Decide F1 (protocol contract) explicitly — current behavior is unintentional. Defer F4 until the transport roadmap is decided.
