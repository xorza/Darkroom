# Design review: prism/src/ top-level wiring  (2026-04-25)

Scope: how the top-level pieces (`main.rs`, `init.rs`, `config.rs`, `app_config.rs`, `ui_host.rs`, `session.rs`) wire `gui/`, `tui/`, `script/`, and `model/` together. Per-submodule internals are out of scope — see future per-submodule reviews for those.

## Current design

`main.rs` parses CLI flags into an `AppConfig` (currently a one-field aggregator wrapping `ScriptConfig`) and dispatches to either `gui::run` or `tui::run`. Each frontend constructs its own driver (`GuiApp` / `TuiApp`) which owns a single `Session` plus a frontend-specific "main window" (`MainWindow` / `MainTui`). `Session` is the frontend-agnostic core: it holds the `ViewGraph`, `FuncLib`, undo stack, the `scenarium::Worker` handle, the `ScriptExecutor`, an `argument_values_cache`, a UI-visible `status` log, and a persistent `Config` (last-opened path). Frontends signal back into non-UI code through the `UiHost` trait, which exposes only `request_redraw` and `close_app`.

The per-frame contract Session asks of a frontend (`GuiApp` / `TuiApp`) is a strict ordering: call `pre_frame` → `update_shared_status` (drains `worker_rx` and `script_action_rx` into `status` / `argument_values_cache` / `execution_stats`), then build the UI against `graph_context()` (immutable view), then call `handle_output(FrameOutput)` (applies queued `GraphUiAction`s, pushes them onto the undo stack, sends `WorkerMessage::Update`/`Run`/`StartEventLoop` if appropriate). On exit, the frontend calls `Session::exit()` which saves `Config`, tells the worker to exit, and explicitly drops the `ScriptExecutor` to release sockets.

Worker→Session communication is funneled through a single `WorkerEvent` enum and one `unbounded_channel`; the executor callback closes over the sender + `UiHost::request_redraw`. Script→Session is symmetric: a separate `ScriptAction` channel drained in the same loop. The two pump methods (`update_shared_status`, `handle_output`) bracket frame rendering on purpose — events must land before render, output must be drained after.

## Overall take

The bones are right. `UiHost` as the only frontend→core capability is a clean cut, the single-channel `WorkerEvent` design is the right shape, and routing all `ViewGraph` mutations through `commit_actions` (renderer holds `&ViewGraph`) is a strong invariant that makes egui's multi-pass rendering safe. The smells below are mostly local: an autorun field with two writers, a `MainWindow` that holds its own `EguiUiHost` parallel to Session's, a Session test stub that hand-lists every field, and one premature aggregator. None of them require structural change — they're all 1–3/5 effort.

## Findings

### [F1] `Session::autorun` has two writers and one canonical owner elsewhere

- **Category**: State
- **Impact**: 4/5 — removes a class of "asserts trip after restart" bugs and a quiet truth-source split
- **Effort**: 2/5 — delete the field, add a method, fix the few read sites
- **Current**: `autorun: bool` lives on `Session` (session.rs:51). It is written in two places: (a) `update_shared_status` recomputes it from `worker.is_event_loop_started()` every frame (session.rs:212–216); (b) `handle_output` toggles it on `RunCommand::Start/StopAutorun` and asserts the previous value (session.rs:295–303). Reads are at session.rs:120, 139, 289 and `MainWindow` (main_window.rs:187), forwarded into `GraphContext` (graph_ctx.rs:19) and consumed by overlay buttons.
- **Problem**: The canonical truth is `Worker::is_event_loop_started()`. The cached field exists only to give synchronous reads in the same frame as a button click. The two writers create a subtle invariant: the assert at session.rs:295 will fire if `update_shared_status` ever runs *between* a `RunCommand::StartAutorun` being queued and `handle_output` consuming it (currently impossible by frame ordering, but it's a load-bearing guarantee with no test). Worse, the field is initialized to `false` (session.rs:98, 433) and only gets reconciled on the first `update_shared_status` — so callers reading `session.autorun()` before the first pump tick see a lie.
- **Alternative**: Delete the field. Replace `pub fn autorun(&self) -> bool` with `self.worker.as_ref().is_some_and(Worker::is_event_loop_started)`. Drop the asserts; `handle_output` just sends the message and lets the worker be the source of truth. Optimistic UI (button toggles before worker confirms) can be handled by the overlay's local `mut autorun = autorun;` shadow that already exists (overlays.rs:24) — that's already the pattern.
- **Recommendation**: Do it.

### [F2] ~~`MainWindow` carries its own `EguiUiHost` parallel to Session's~~ — **Rejected**

Rejected by user (2026-04-25). Keeping `MainWindow`'s own `EguiUiHost` is intentional: it preserves the invariant that UI modules talk to the frontend exclusively through `UiHost`, even when egui's `Context` is in scope. Reaching for `gui.ctx().send_viewport_cmd(...)` would inline egui-specific calls inside `MainWindow`, making it impossible to reuse `MainWindow` behind a non-egui `UiHost` later. The duplicate `EguiUiHost` clone is cheap (one `Context` Arc-clone) and the consistency is worth more than the dedup.

### [F3] Session test stub re-lists every field and silently diverges from `Session::new`

- **Category**: Contract / Maintenance
- **Impact**: 3/5 — every new Session field must be added in two places or tests use a stale shape; already true that `test_session()` skips `Config::load_or_default` and `func_lib` registration
- **Effort**: 2/5 — extract a `Session::new_for_test(ui_host, config)` constructor (or a small builder) that production `new` also funnels through
- **Current**: `tests::test_session` (session.rs:423–443) builds `Session { … }` field-by-field with `worker: None`, `script_executor: None`, default `FuncLib`, default `Config`. Production `Session::new` (session.rs:67–114) does the real construction with worker spawn, funclib merge, config autoload.
- **Problem**: This is the classic "two constructors that must stay in sync" trap. Adding a field to `Session` compiles fine while leaving the test stub broken in ways tests don't catch (they don't exercise the new field). Worse, the test stub is the only callable path that produces a `Session` with `worker: None` — see F4 — and the `Option<Worker>` exists in production solely to make this stub possible.
- **Alternative**: A constructor like `Session::with_components(ui_host, func_lib, config, worker, script_executor)` that both `new` and `new_for_test` delegate to. Or a small `SessionBuilder` if the parameter list bloats. Keeps initialization in one place and lets tests vary only what they need.
- **Recommendation**: Do it. Pairs naturally with F4.

### [F4] `Option<Worker>` and `Option<ScriptExecutor>` carry two different meanings

- **Category**: Types
- **Impact**: 3/5 — reading code, both look like "may be absent for legitimate runtime reasons" but they encode different things
- **Effort**: 2/5
- **Current**: `worker: Option<Worker>` exists because the test stub (F3) wants to skip spawning a worker — every production read does `if let Some(worker) = &self.worker` (session.rs:336, 359) or `is_some_and` (session.rs:213). `script_executor: Option<ScriptExecutor>` exists so `Session::exit` can write `self.script_executor = None;` (session.rs:351) to drop it explicitly before `eframe::run_native` returns, releasing sockets and the discovery file.
- **Problem**: Same shape, different reason. A reader has to puzzle out "can the worker be `None` at runtime?" (no, only in tests) vs "can the executor be `None`?" (yes, briefly, between `exit()` and Drop). Conflating the two means you can't tighten one without the other. The `Option<Worker>` is also test-shaped state leaking into production layout.
- **Alternative**: After F3, `worker: Worker` (no Option) — tests use a real Worker or a no-op stub conforming to a small trait. For `script_executor`, either (a) leave the Option but rename to make intent local: `script_executor_handle` with a doc that "set to None during exit to release sockets", or (b) make `Session::exit` consume self (`pub fn exit(self)`) and have `GuiApp::on_exit` move out via `Option<Session>`. Option (a) is the cheap fix; (b) is cleaner but ripples to GuiApp.
- **Recommendation**: Do (a) once F3 lands. Skip (b) unless you find another reason to want owned exit.

### [F5] Pre-frame / post-frame split on Session is a load-bearing ordering contract

- **Category**: Contract
- **Impact**: 2/5 — works today; fragile if a third frontend appears
- **Effort**: 2/5
- **Current**: Frontends must call `Session::update_shared_status()` *before* rendering and `Session::handle_output()` *after* — see `MainWindow::pre_frame` (gui/app.rs:26, main_window.rs:84) and `MainWindow::render` (main_window.rs:111). Nothing enforces or documents the ordering at the Session API level. `TuiApp` doesn't render against Session either way and gets away with calling neither.
- **Problem**: A new frontend that forgot `update_shared_status` would render against stale `execution_stats` and never advance the `argument_values_cache`. The function names don't telegraph "drain inbound" vs "drain outbound" — `update_shared_status` reads like a status-string thing.
- **Alternative**: Either rename to `Session::drain_inbound()` / `Session::handle_frame_output(output)`, or fold into `Session::tick(output) -> ()` that internally drains inbound first, returns a borrow for the renderer, and consumes output last. The latter only works if the borrow checker cooperates — Session needs to expose `graph_context()` between the two halves, so a single `tick` call probably doesn't fit. Renaming + a doc note on `Session` is enough.
- **Recommendation**: Rename + doc. Don't restructure.

### [F6] ~~`AppConfig` is a one-field aggregator justified by future fields~~ — **Rejected**

Rejected by user (2026-04-25). Keeping `AppConfig` as the frontend-config seam: window geometry, recent files, theme overrides, and similar runtime knobs are concretely planned to land here, and growing the struct in place beats threading new arguments through `gui::run` / `tui::run` each time. The CLAUDE.md "no speculative generalization" rule applies to abstractions invented without a use case; here the use case is on the near roadmap.

### [F7] `Session::replace_graph(reset_undo: bool)` — bool param always passed `true`

- **Category**: Generalization
- **Impact**: 1/5
- **Effort**: 1/5
- **Current**: `replace_graph(view_graph, reset_undo: bool)` (session.rs:354) — both call sites (`empty_graph` line 171, `read_graph_from` line 207) pass `true`.
- **Problem**: An unused parameter that future callers might pass `false` to without anyone having thought through what "replace the graph but keep the undo stack pointing at the old one" means.
- **Alternative**: Inline the bool — `replace_graph` always resets undo. If a non-resetting variant ever becomes real, name it then.
- **Recommendation**: Do it. Trivial.

## Considered and rejected

- **Make `UiHost` an enum instead of a trait** — only two impls today, but the trait boundary is also what lets `Session::new` be generic over a test no-op host (session.rs:67, 416). Trait wins.
- **Move `Config` (last-opened path) onto `AppConfig`** — Config is mutated at runtime (set on save/load) and persisted on exit; AppConfig is parsed-once and immutable. Different lifecycles; keep separate.
- **Collapse `WorkerEvent` and `ScriptAction` into one channel** — they have distinct producers (worker callback vs tokio executor task) and distinct semantics (results-of-compute vs side-effect requests). Conflating would require a discriminant variant for "which side asked" that the current split avoids. Keep separate.
