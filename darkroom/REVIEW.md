# Darkroom architecture review — 2026-07-24

## Scope and flow

This review covers production code in `darkroom`; tests, fixtures, and benchmark
code were ignored. Aperture and Scenarium were followed only where Darkroom's
correctness or ownership claims depend on them.

`Document` owns the authored graph, graph views, and dock layout. GUI and script
inputs become `Intent`s, which the edit layer turns into reversible `UndoStep`s
and stores in `ActionStack`. `Workspace` pairs the open document with
`RuntimeHost`; the latter composes the runtime library, compiles graphs, sends
commands to the worker, and surfaces worker/script reports. The GUI projects the
active graph into a rebuilt `Scene`, then records dock, canvas, node, inspector,
and viewer surfaces. TUI and headless modes drive the same workspace through
`TerminalSession`.

The most serious weaknesses are at trust and identity boundaries: recursive
graphs are addressed with an insufficient identifier, externally decoded edits
are trusted as if they came from constrained widgets, destructive document
transitions do not share one dirty-state policy, and script-side limits do not
bound host-side memory.

## Critical

- [ ] **A bare local-graph ID cannot address Darkroom's recursive graph model.**
  `GraphRef::Local(GraphId)` resolves only through the root graph's `graphs` map,
  while adding a graph instance inside a local graph inserts its definition into
  that local graph's own map (`src/core/document/mod.rs:25-31`,
  `src/core/document/mod.rs:293-326`, `src/core/edit/intent/apply.rs:177-200`).
  The node badge still emits only the bare ID, and opening then silently fails
  root lookup (`src/gui/node/prepass.rs:33-41`,
  `src/gui/app/editor/mod.rs:459-475`). `Graph::fresh_copy` preserves nested
  graph IDs, so copied recursive subgraphs can also make the same ID ambiguous
  across different holders (`../scenarium/src/graph/clone.rs:54-56`). Nested
  composites deeper than one level are consequently present in the authored
  model but cannot be navigated or edited reliably.

- [ ] **The remote scripting surface exposes every internal intent while the
  edit layer trusts widget-enforced invariants.** `apply` and `apply_all`
  deserialize arbitrary Rhai values into any `Intent` variant
  (`src/core/script/engine.rs:130-155`). `AddNode` accepts an existing node ID
  during step construction and then asserts that it is absent during apply
  (`src/core/edit/intent/build.rs:120-135`,
  `src/core/edit/intent/apply.rs:177-187`). `SetInput` validates only the
  destination node before accepting any producer, and `SetSelection` accepts
  arbitrary item identities (`src/core/edit/intent/build.rs:193-204`,
  `src/core/edit/intent/apply.rs:245-250`). A syntactically valid remote request
  can therefore panic the GUI/headless host or create graph/view state rejected
  by Darkroom's own validator (`src/core/document/validate.rs:75-90`).

- [ ] **Script resource limits do not bound host-side output, effect queues, or
  replies.** Each `print` appends to an uncapped host `String` and clones the
  message into an unbounded host channel (`src/core/script/engine.rs:82-96`,
  `src/core/script/mod.rs:147-173`). One script may execute ten million Rhai
  operations, allowing a print/effect loop to queue millions of allocations
  while the host is not draining (`src/core/script/engine.rs:18-29`). The TCP
  reader enforces `MAX_FRAME_BYTES`, but reply serialization and compression
  have only a `u32` length conversion and no corresponding cap
  (`src/core/script/tcp/mod.rs:408-443`). A valid client can exhaust host memory
  and produce outbound frames far larger than the protocol's stated limit.

- [ ] **Dirty-document protection is fragmented across destructive
  transitions.** New and Load replace the entire editor/document without
  consulting dirty state (`src/gui/app/commands/file.rs:25-70`), and script
  `shutdown()` calls the host quit path directly
  (`src/gui/app/mod.rs:155-177`). In the exit dialog, “Don't ask again” is
  persisted before Save succeeds, so a cancelled or failed Save As leaves a
  dirty document open with future confirmation disabled
  (`src/gui/app/mod.rs:247-263`, `src/gui/app/commands/file.rs:77-103`).
  Ordinary UI and authenticated script actions can therefore discard authored
  work without a final confirmation.

## High

- [ ] **Undo can split local-graph definitions from their persisted views.**
  `Document` stores graph definitions, `local_views`, and layout independently
  (`src/core/document/mod.rs:260-274`). Reverting `AddNode` or `DetachGraph`
  removes a nested graph but leaves any lazily created `local_views` entry
  (`src/core/edit/intent/apply.rs:410-414`,
  `src/core/edit/intent/apply.rs:481-493`). Layout repair prunes dead tabs but
  never orphaned views (`src/core/document/mod.rs:407-435`), while validation
  rejects an orphan view (`src/core/document/validate.rs:111-121`). A normal
  open-then-undo sequence can leave the document structurally invalid.

- [ ] **Release saves can persist documents that the next load refuses.**
  Saving calls only `validate_debug` before serialization
  (`src/core/io/document.rs:180-189`), and that method returns immediately in a
  non-debug build (`src/core/document/validate.rs:133-140`). Loading always runs
  full validation (`src/core/io/document.rs:166-177`). Any latent invariant
  violation can therefore be written as an apparently successful project and
  discovered only when the file is reopened.

- [ ] **Graph-library mutations become live before durable persistence
  succeeds.** Import mutates the owned library map first, while publish mutates
  both the library and the document's lineage metadata
  (`src/core/runtime_library/mod.rs:70-84`,
  `src/core/edit/publish.rs:10-41`). The finish path records a save error but
  still recomposes and publishes the changed runtime library
  (`src/core/runtime_library/mod.rs:88-99`). A graph can work for the rest of
  the session and disappear on restart, while the open document retains an
  origin for an entry that never reached disk.

- [ ] **The process-global graph library has a lost-update race across
  Darkroom instances.** Each process retains a private library snapshot
  (`src/core/runtime_library/mod.rs:37-68`). Before writing, the persistence
  path re-reads the file only to prove it is parseable, discards that newer
  value, and replaces the whole file with the caller's snapshot
  (`src/core/io/graph_library/mod.rs:144-159`). Two live instances can both
  report a successful import/publication while the later save silently erases
  the other process's changes.

- [ ] **Worker command delivery failures are discarded while runtime methods
  report success.** Install, run, cache, and event-loop sends drop their
  `Result`s (`src/core/worker.rs:59-115`). `RuntimeHost` returns `true` solely
  because compilation succeeded (`src/core/runtime_host.rs:150-186`,
  `src/core/runtime_host.rs:195-205`). If the worker task has exited, callers
  are told that work was queued, local cache projections may be cleared, and no
  report can arrive to reconcile or expose the failure.

- [ ] **Retained canvas gestures can commit identities invalidated after the
  gesture began.** Rubber-band selection snapshots the initial selection and
  writes it back verbatim on release
  (`src/gui/canvas/selection_ui.rs:24-29`,
  `src/gui/canvas/selection_ui.rs:89-140`); undo runs before the gesture
  prepass and can remove one of those items (`src/gui/app/editor/mod.rs:314-321`).
  A held wire similarly retains only its starting `PortRef`, treats missing type
  information as compatible, and can emit a binding after that producer was
  removed (`src/gui/canvas/connection_ui.rs:39-48`,
  `src/gui/canvas/connection_ui.rs:451-473`,
  `src/gui/canvas/connection_ui.rs:525-547`). The edit boundary accepts both
  payloads, producing missing selection members or dangling producers that
  graph validation rejects.

- [ ] **Pinned-output identity collapses distinct execution occurrences.**
  Worker output identity is reduced from `ExecutionNodeId` to the authored leaf
  `NodeId`, then stored solely by authored `OutputPort`
  (`src/gui/run_state.rs:288-302`,
  `src/gui/pinned_output.rs:57-75`). Multiple instances of the same local graph
  therefore race into one preview/viewer entry; the displayed value is whichever
  occurrence reported last, with no stable indication of which instance
  produced it.

- [ ] **All viewer tabs eagerly retain full images, including inactive panes.**
  Reconciliation enumerates every dock tab and materializes every viewer source
  (`src/core/document/mod.rs:362-370`,
  `src/gui/pinned_output.rs:78-111`). Materialization performs GPU readback,
  CPU conversion/downscaling, and texture registration synchronously on the GUI
  thread (`src/gui/pinned_output.rs:115-159`) and retains textures up to
  8192×8192 RGBA, roughly 256 MiB each
  (`src/gui/pinned_output.rs:18-19`). Restored or dormant tabs can stall a frame
  and exhaust memory without ever becoming visible.

- [ ] **Live status patches overwrite rather than aggregate repeated authored
  nodes.** `RunState` states that an authored interior node represents every
  flattened occurrence, but patch handling directly assigns each occurrence's
  status (`src/gui/run_state.rs:5-11`,
  `src/gui/run_state.rs:163-187`). Completed snapshots use the separate merged
  path (`src/gui/run_state.rs:238-258`). During a run, a later `Running` or
  `Executed` patch can hide an earlier `Errored` or `MissingInputs` occurrence
  until completion, making the definition tab's live status misleading.

- [ ] **Delayed dock actions use positional identities after history may have
  changed the layout.** Navigation applies undo/redo before scanning
  last-frame tab responses (`src/gui/app/editor/mod.rs:314-331`), while close
  and activation actions carry `(TabGroupId, index)`
  (`src/gui/dock/mod.rs:82-112`,
  `src/core/document/dock.rs:185-195`). A stale click can act on a new occupant.
  An in-flight tab drag also retains the old group/index and polls that
  index-derived widget after undo can move the tab
  (`src/gui/dock/drag.rs:19-27`, `src/gui/dock/mod.rs:130-149`), leaving the
  gesture latched against a widget that no longer exists.

## Medium

- [ ] **Automation infers graph scope from persisted GUI navigation state.**
  The same generic script API exposes graph edits together with selection,
  viewport, and dock intents (`src/core/script/engine.rs:130-158`).
  Terminal/headless execution chooses a target from the document's active
  primary tab and falls back to Main (`src/core/terminal_session/mod.rs:103-114`,
  `src/core/document/mod.rs:343-351`). A headless edit can therefore target a
  different graph solely because of whichever tab a previous GUI session left
  open, and automation can mutate presentation-only state.

- [ ] **One untyped sticky-error slot lets unrelated success erase unresolved
  failures.** `StatusLog` promises same-family clearing but stores only one
  `Option<String>` (`src/core/status.rs:14-23`). Compile success, library
  success, completed runs, and file success all assign `None` directly
  (`src/core/runtime_host.rs:101-109`,
  `src/core/runtime_host.rs:125-136`,
  `src/core/terminal_session/mod.rs:74-99`,
  `src/gui/app/commands/file.rs:55-103`). A successful unrelated action can
  hide a persistence or runtime failure whose underlying condition remains.

- [ ] **The frame-wide `Option<AppCommand>` drops co-occurring valid
  actions.** Independent surfaces repeatedly overwrite the same optional
  command, and App dispatches only the survivor
  (`src/gui/main_window.rs:76-136`, `src/gui/app/mod.rs:328-330`). In one
  concrete path, committing an edited ML path stores `Changed`, then clicking
  Browse overwrites it with `PickMlModel`
  (`src/gui/preferences_view.rs:273-298`). Cancelling the picker leaves the
  in-memory path changed without the reconfiguration or persistence attached to
  the lost command.

- [ ] **Viewer tabs persist port references that are not validated as live
  outputs.** `TabRef::ImageViewer` stores a general `PortRef`, but tab liveness
  checks only node existence (`src/core/document/mod.rs:61-84`,
  `src/core/document/mod.rs:280-287`). Document validation consequently accepts
  input-kind and out-of-range viewer tabs
  (`src/core/document/validate.rs:123-129`). Resource retention ignores an
  input-kind tab while rendering reinterprets the same index as an output
  (`src/core/document/mod.rs:362-370`,
  `src/gui/main_window.rs:122-136`), so persisted state has contradictory
  meanings across subsystems.

- [ ] **Canvas geometry caches can both leak identities and make stale bounds
  self-perpetuating.** Persistent node-size and port-offset maps survive graph
  switches and clear only their live subsets
  (`src/gui/canvas/geometry.rs:45-70`,
  `src/gui/canvas/geometry.rs:176-188`). Repeated node/interface churn grows
  those maps for the editor's lifetime. Culling uses the cached node size before
  recording (`src/gui/node/mod.rs:144-153`), so an off-screen node whose dynamic
  content grows into the viewport can remain skipped forever and never refresh
  the stale size.

- [ ] **Pinned inspectors are deleted on a graph switch despite living outside
  resettable gesture state.** `GraphUI` keeps inspector state across target
  resets (`src/gui/canvas/mod.rs:76-108`), but `Inspectors::apply` retains only
  node IDs in the currently active `Scene`
  (`src/gui/canvas/inspector.rs:98-120`). Visiting another graph permanently
  removes every pinned inspector from the previous graph.

- [ ] **Idle graph frames rebuild semantic projection state and allocate in
  proportion to graph size.** `Editor` unconditionally rebuilds the active
  scene (`src/gui/app/editor/mod.rs:206-217`); `Scene::rebuild` clears and
  repopulates every pool, rematches every `NodeKind`, clones constants/types,
  and creates temporary event vectors (`src/gui/scene.rs:251-330`,
  `src/gui/scene.rs:410-449`). Visible node recording adds fresh row, tooltip,
  and option vectors/strings (`src/gui/node/port_row/mod.rs:57-88`,
  `src/gui/node/port_row/mod.rs:208-218`,
  `src/gui/node/value_editor.rs:62-84`). During execution this work is forced at
  roughly 20 fps, so canvas culling reduces drawing but not the graph-sized
  projection and allocator load (`src/gui/app/mod.rs:308-314`).

- [ ] **The new-node palette rebuilds its catalogue with
  category-times-entry work on every open frame.** It reconstructs and sorts
  categories, then rescans every function, special node, and graph once per
  category into fresh vectors with allocated lowercase keys
  (`src/gui/canvas/new_node_ui.rs:189-196`,
  `src/gui/canvas/new_node_ui.rs:217-252`,
  `src/gui/canvas/new_node_ui.rs:289-296`). Catalogue work is `O(C·N)` per
  repaint and creates transient memory proportional to the library even when
  neither the library nor query changed.

- [ ] **Core's declared frontend boundary is violated by persisted GUI
  preferences.** `core` promises no Aperture dependency, but its preference
  schema imports and serializes `aperture::ImageFilter`
  (`src/core/mod.rs:1-5`, `src/core/io/preferences.rs:1-3`,
  `src/core/io/preferences.rs:67-75`). `TerminalSession` must load the same
  GUI-coupled type (`src/core/terminal_session/mod.rs:26-30`), tying headless
  startup and the persistent core schema to a GUI library enum.

- [ ] **The dock model pays indexed-arena complexity for a tree capped at 31
  nodes.** A depth-four cap bounds the layout, yet the representation
  serializes `NodeIdx` edges, requires canonical preorder storage, repacks after
  structural changes, and validates reachability/index topology
  (`src/core/document/dock.rs:9-26`,
  `src/core/document/dock.rs:245-267`,
  `src/core/document/dock.rs:396-456`,
  `src/core/document/dock.rs:548-677`). The mutation and persistence surface is
  disproportionate to the small maximum structure and makes positional-address
  bugs harder to isolate.

- [ ] **Configuration recovery conflates absence, transient failure, and
  corruption, then mutates recovery state.** Preferences map missing,
  unreadable, and malformed files to defaults
  (`src/core/io/preferences.rs:148-155`), and preferred-document load failure
  clears and persists the remembered path even for a temporarily unavailable
  drive or permission error (`src/core/workspace/mod.rs:18-43`). Graph-library
  quarantine always uses one `.broken` target
  (`src/core/io/graph_library/mod.rs:77-84`,
  `src/core/io/graph_library/mod.rs:124-140`), so a second failure can overwrite
  the first recovery copy on platforms where rename replaces destinations or
  fail to quarantine at all on others.

- [ ] **Edit-time graph algorithms scale poorly on interactive paths.**
  Multi-selection drag coalescing maps every previous move and linearly scans
  the current moves for its partner, yielding `O(N²)` comparisons per input
  frame (`src/core/edit/action_stack/mod.rs:99-105`,
  `src/core/edit/intent/query.rs:193-217`). First-open auto-layout may scan all
  edges up to `V−1` times and runs synchronously while opening a local graph
  (`src/core/document/auto_layout.rs:31-46`,
  `src/core/document/mod.rs:377-393`). Large selections and reverse-ordered
  graphs can therefore stall otherwise direct manipulation/navigation.

- [ ] **Every frontend and optional scripting stack is compiled into one
  unconditional binary dependency graph.** `main.rs` declares GUI, TUI, and
  headless modules together, while desktop, Rhai, TCP compression, and tracing
  dependencies are unconditional (`src/main.rs:1-16`, `Cargo.toml:7-39`).
  Headless and non-script builds retain avoidable desktop/script compile edges;
  `WorkerBridge` and optional `ScriptHost` also each own a separate multithread
  Tokio runtime (`src/core/background_runtime.rs:1-35`,
  `src/core/worker.rs:37-50`, `src/core/script/mod.rs:343-396`), multiplying
  threads and shutdown ownership inside one process.

## Low

- [ ] **Terminal commands collapse distinct outcomes into misleading
  messages.** `TerminalSession::save` returns the same `false` for an untitled
  document and an I/O failure (`src/core/terminal_session/mod.rs:58-70`), while
  TUI always reports “nothing to save.” `run` ignores whether compilation and
  queueing succeeded and always prints “run queued”
  (`src/tui/mod.rs:60-69`). Operators receive success/no-op text for real
  failures unless they separately inspect status history.

- [ ] **Script CLI input is silently ignored or rewritten.** Script bind,
  token, auth, and token-file flags have no effect unless `--script-tcp` is
  also present, and an invalid bind specification logs a warning then falls
  back to the default address (`src/main.rs:55-99`). A mistyped invocation can
  start with materially different listener/authentication behavior than its
  arguments imply.

- [ ] **Terminal errors bypass the teardown structure built around runtime
  drop order.** `run_terminal` deliberately keeps `TerminalSession` outside the
  async block so worker/script runtimes drop in synchronous context, but its
  error branch calls `std::process::exit(1)`
  (`src/main.rs:195-218`). Destructors never run on that path, abandoning the
  cooperative task, socket, and runtime cleanup the surrounding ownership was
  designed to provide.

- [ ] **Floating-point document state claims `Eq`.** `GraphView` contains
  `f32` viewport and position values but manually implements `Eq`; `Document`
  then does the same and also contains dock split ratios
  (`src/core/document/mod.rs:130-198`,
  `src/core/document/mod.rs:514`,
  `src/core/document/dock.rs:232-237`). NaN violates `Eq` reflexivity, so these
  trait implementations promise a contract the persisted value domain does not
  satisfy.

## Open questions

- [ ] **The packed undo arena has substantial complexity without cited
  workload measurements.** Every history entry is serialized/deserialized into
  a one-MiB byte arena with byte ranges, head offsets, lazy compaction, and
  parallel metadata (`src/core/edit/action_stack/mod.rs:1-64`,
  `src/core/edit/action_stack/mod.rs:71-267`). No benchmark in the reviewed
  module establishes that this representation materially improves memory or
  latency for realistic edits.

- [ ] **The one-entry project archive carries a full ZIP parsing and validation
  surface.** `.darkroom` contains exactly one bounded `document.json`, but load
  validates archive topology, duplicate entries, overlap, entry kind, and two
  sizes before parsing it (`src/core/io/document.rs:1-16`,
  `src/core/io/document.rs:74-147`). The reviewed code contains no product
  requirement for additional archive entries that explains this format and
  dependency weight.

- [ ] **Persistent Rhai sessions have no retained-memory budget.** Up to 32
  sessions keep their `Scope` for ten idle minutes
  (`src/core/script/session/mod.rs:17-26`,
  `src/core/script/session/mod.rs:51-81`). Per-evaluation string/collection and
  variable-count limits do not bound the aggregate object graph retained across
  successive valid requests, and no workload requirement in the module
  establishes the needed persistence window or memory envelope.
