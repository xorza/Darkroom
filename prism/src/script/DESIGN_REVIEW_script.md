# Design review: `prism/src/script/`  (2026-04-24)

## Current design

A single tokio task owns one `rhai::Engine` and processes `ScriptRequest`s off a bounded mpsc queue. Transports (only `TcpTransport` today) bind their sockets eagerly — so the caller learns OS-assigned ports before the accept loop starts — and push requests into the shared queue. Each request carries an `origin: String` identifying the sender so `print` output can be attributed when it reaches `Session`.

The `on_print` Rhai hook is registered once on the engine; to make it per-request-aware it reads from an `Arc<Mutex<RequestState>>` (origin + stdout accumulator). Before each `run_script` call the executor sets `origin`, runs `eval_with_scope::<Dynamic>`, serializes the final value via `common::serde_rhai::to_string`, then `mem::take`s the accumulated stdout into a `ScriptResult`. `ScriptResult` derives `Serialize`/`Deserialize`, so the TCP transport's reply is just `serde_json::to_string(&reply)` — the Rust struct *is* the wire shape.

Cancellation: every transport owns a `CancellationToken`; the executor owns another. Dropping `ScriptExecutor` cancels the executor and all transports. Per-connection tokens are constant-time-compared (branchless u128 XOR) and failures close the connection silently.

## Overall take

The module is in a good shape after its last several rounds of refactoring. Pure rendering is split from I/O (`render_token_file`), the wire contract lives in the derived struct rather than a hand-assembled JSON payload, and the transport boot path is side-effect-free until it returns to `build_transports`. **One significant finding below** concerns an implicit invariant of the executor loop that could be a subtle correctness/security issue depending on intent; the rest are minor polish.

## Findings

### [F1] `Scope` is shared across all requests, leaking state between clients

- **Category**: State / Contract
- **Impact**: 4/5 — potential cross-client state leakage and unbounded scope growth, depending on whether shared state is intended
- **Effort**: 1/5 — move `let mut scope = Scope::new();` inside the accept loop
- **Current**: `run_executor` (mod.rs:283–303) creates `let mut scope = Scope::new();` **outside** the loop, then passes `&mut scope` to every `run_script` call. Rhai's `Scope` persists `let` bindings across script runs, so:
  ```rust
  // Client A connects, sends:   let secret = 42;
  // Client B connects, sends:   secret    -> returns 42
  ```
- **Problem**: the TCP protocol auth is per-connection (each client must present a valid UUID), but *script state* is global. A compromised or lower-trust client can read variables left behind by a higher-trust one. Additionally, `MAX_VARIABLES = 256` becomes a session-wide budget — a long-running prism dies after 256 cumulative `let`s across all clients, with no clear error message (Rhai's limit exceeded fires mid-script). Nothing in the module's doc or the TCP protocol doc mentions this lifetime; the only way to learn it is to read the executor loop and know Rhai's `Scope` semantics.
- **Alternative A (per-request scope, stateless)**: move `let mut scope = Scope::new();` inside the accept loop body. Each script starts fresh. The `MAX_VARIABLES` cap now applies per-script, which is what the comment on line 116 implies ("256 is ample for any legitimate script" — legitimate single script, not a session).
- **Alternative B (explicit REPL mode)**: keep shared scope but surface it. `ScriptRequest` gains a `session: Option<SessionId>`; executor maintains a `HashMap<SessionId, Scope>`. Anonymous requests (no session id) get a fresh scope. Explicit REPL clients opt in.
- **Recommendation**: **Do A now.** The current behavior is almost certainly unintentional — the module doc describes "running a script" singular, the TCP protocol is one-script-per-connection, and there's no client API to address an existing session. If REPL support is wanted later, B is additive.

### [F2] Response assembly is split between `run_executor` and `run_script`

- **Category**: Control flow / Responsibility
- **Impact**: 2/5 — readability; the split forces a reader to track two functions to understand one reply
- **Effort**: 1/5 — local to `run_executor` + `run_script`
- **Current**: `run_script` returns `Result<String, String>` (mod.rs:315–321). `run_executor` unwraps that into `(Option<String>, Option<String>)`, then reads `state.stdout` from the mutex and assembles the final `ScriptResult` (mod.rs:294–299). The `Result<String, String>` type signature can't tell a reader which string is the serialized result and which is the error — they have to read the body.
- **Problem**: two functions collaborate to build one reply, with the shape of the reply half-encoded in both. Moving forward, changes to `ScriptResult` (add a field, change semantics) must be made in two places.
- **Alternative**: `run_script` becomes `fn run_script(engine, scope, state, source) -> ScriptResult`. It performs the eval, the serialization, and the `mem::take` in one place. `run_executor` collapses to:
  ```rust
  let reply = run_script(&engine, &mut scope, &state, &req.source);
  let _ = req.reply.send(reply);
  ```
- **Recommendation**: Do it. Small, improves locality.

### [F3] `stdout.clear()` is redundant

- **Category**: Control flow (trivia)
- **Impact**: 1/5 — one dead instruction
- **Effort**: 1/5
- **Current**: `run_executor` does `s.stdout.clear();` at the top of each iteration (mod.rs:292). The prior iteration ended with `std::mem::take(&mut state.lock().unwrap().stdout)` (mod.rs:297), which leaves the buffer empty. Nothing writes to `stdout` between iterations because `on_print` is only invoked during `engine.eval_with_scope`.
- **Problem**: the `clear` is belt-and-braces that doesn't defend against anything reachable. A reader has to trace the execution flow to be sure.
- **Alternative**: delete the line.
- **Recommendation**: Do it when touching this function anyway.

## Considered and rejected

- **`ScriptTransport` trait with one implementor** — raised in the previous review as F4; user decided to keep for anticipated future transports. Still holds.
- **`ScriptAction` enum with one variant** — same rationale as above.
- **Intern `origin` as `Arc<str>`** — avoids cloning a ~40-char string per `print()` call. Real scripts print 0–10 times; unmeasurable savings.
- **Move serialization off the executor task** — `serde_rhai::to_string` runs on the single executor task, blocking the next script. Bounded by `MAX_MAP_LEN * MAX_STRING_SIZE`-ish which is finite but not tiny. For the current single-client, single-task use case it's fine; if concurrent TCP throughput ever becomes a goal, serialize in a per-request spawn.
- **Derive serialization shape (`JSON`) into `ScriptResult` via a transport-neutral method** — current arrangement (TCP transport calls `serde_json::to_string(&reply)` directly) is simple and a second transport is speculative.

## Scorecard

| # | Finding | Impact | Effort |
|---|---|---|---|
| F1 | `Scope` is shared across all requests | 4 | 1 |
| F2 | Response assembly split across `run_executor`/`run_script` | 2 | 1 |
| F3 | Redundant `stdout.clear()` | 1 | 1 |

F1 is the one that matters — decide explicitly whether per-request scope or a REPL-style shared scope is the intended contract. F2 + F3 can fold into the same commit.
