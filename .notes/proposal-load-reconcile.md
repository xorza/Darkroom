# Proposal: reconcile a document at load, assert after

A document can hold state that does not fit the library or itself. Today each
reader of the authoring graph tolerates that state in its own way: the compile
lowers a bad binding as unbound, the editor reads it again each frame, and the
two disagreed. The editor locked the Detection and Registration pickers of
`stack_lights`, because `Graph::overridden` counted a mistyped constant on a
Config input as an override, while the run ignored it.

The convention is now in `AGENTS.md` ("A document is parsed once, at load"):
the load handles each inconsistency and reports it, the edit gate keeps the
document consistent, and run time and each frame only assert. This proposal
lists every inconsistency a load can meet, every part of the wildcard and stub
machinery, and every run-time check of the document, and says what happens to
each.

## Decisions

- **Reconcile at load, assert after.** The library is fixed for the duration
  of a process: a recompose (`RuntimeLibrary::update_ml_model_paths`) changes
  only the default of the ML "Model" input, and the signature does not include
  defaults. Thus only a load can bring in a mismatch with the library. Refused:
  tolerance at each reader, because two readers disagreed. Source: the practice
  "parse, don't validate".
- **A bad binding at load is removed and reported.** The load removes each
  binding and subscription that a node cannot hold, reports each one, and marks
  the document dirty, so a save accepts the change. Refused: to refuse the
  document, because the file then does not open. Also refused: to reset to the
  port default, because the meaning changes with no warning.
- **A matching signature is not enough.** The signature covers port names,
  types, order and event names. It does not cover the `required` flag,
  `const_only`, picker `value_variants`, `overrides`, an enum's variant set or a
  default (`graph/tests.rs`, `a_signature_covers_the_ports_and_nothing_else`).
  A library change to any of these keeps a node resolved but can break one of
  its bindings, so the load checks each binding of a resolved node too.
- **Stubs are darkroom's, not scenarium's.** A stub keeps your work while the
  library cannot run it; that is an editor concern. `Document.stubs` holds each
  stub as a `DetachedNode` record — the node with every binding and
  subscription that touched it, which undo already uses to restore a removed
  node. A scenarium `Graph` holds only nodes that resolve, so `FuncResolution`,
  `Node::resolve`, `MissingFunc`, `FuncChanged` and the compile's stub refusal
  go. The signature stays in scenarium: it identifies a node's ports and keys
  the cache. Refused: stubs as graph nodes, because every reader then needs a
  "declares nothing" branch and the compile needs a `Result`; to remove stubs,
  because the node's settings are lost; to remap by port name, because the
  files get bigger and a value whose meaning changed still goes through.
- **A graph with a stub runs the rest.** A consumer that a stub fed sees an
  unbound input — a missing-input verdict if it is required, as with a disabled
  producer. Refused: today's rule that one stub refuses every run.
- **A stub can be selected, moved and deleted; Duplicate skips it.** It has no
  ports, so it shows no wires, as today. A copy of a node that cannot run has
  little use.
- **A stub's record is a frozen snapshot.** Its edges may name nodes that were
  removed later. When the stub resolves at a later load, it is attached again;
  an edge whose other end is gone, or whose input the graph has bound since, is
  removed and reported. When two stubs share an edge, the record of the stub
  detached first holds it.
- **Wildcard outputs are removed.** No production func calls `wildcard_output`;
  only tests do. Without wildcards, an output's type is its declaration, so no
  edit can break a distant wire, and nothing is left to resolve. Refused: an
  edit that removes the broken downstream wires, because you lose wires you did
  not touch. Also refused: an edit that is refused, because you must remove the
  downstream wires by hand first. If reroute nodes come later, wildcards need a
  new design.
- **No output is `Any`.** `FuncBuilder` refuses one (`AnyOutput`), and
  `DataType::compatible_with` passes `Any` on the input side only. An `Any`
  output was the unresolved wildcard: it fed any input, so no gate could
  check its wires. No shipped func declares one.
- **The signature encoding goes to v2.** Without the output-kind tag, every
  signature changes, and documents saved before the change load with their func
  nodes as stubs. There is no compat code. A one-off script outside the repo can
  restamp a file where the v1 signature matches the current ports.
- **One admission rule, in scenarium.** `Graph::admit` is the rule for the
  load, the edit gate and the compile's assert. It replaces
  `Library::binding_satisfies` and darkroom's `core/document/declaration.rs`.
- **The invariant is the host's contract, not a type.** `Graph.bindings` is a
  `pub` field and a `Graph` holds no library, so no type can enforce
  admission. The edit gate is the only way an edit lands.
- **The compile checks its contract with one `assert!`.** With no stubs, the
  compile has no expected failure: it returns `CompiledGraph`, and
  `CompileError` goes. One release `assert!` at its start checks the whole
  contract — each node resolves, each binding and subscription is admitted,
  the structure is whole, no cycle — because `Compiler::compile` is public API
  off the hot path, so a broken contract panics on the UI thread naming the
  bad binding instead of misreading in a lambda on the worker. The checks
  inside the walk and the planner's cycle check stay `debug_assert!`.
- **View and layout state is repaired, not refused.** A missing placement goes
  to the origin; an extra placement or selection entry goes; a non-finite
  position goes to the origin; a bad viewport resets; a viewer tab on a missing
  or non-preview node closes; a wrong pinned tab or dock seed resets the
  layout. Each repair is reported. Refused: refusing the file, because a bad
  viewport alone would keep the graph closed.
- **Graph structure is handled, not refused.** A binding or subscription that
  names a missing node is removed. A node with the nil id is removed with its
  edges, because `Graph::find` promises no graph holds the nil id, so nothing
  can address it. A func node with the nil func id becomes a missing-func
  stub, the state a lookup gives it. Each is reported.
- **A data cycle is broken and reported.** A walk in sorted node-id order
  removes each wire that returns to a node on the current path, and the report
  names each one. Only a hand-edited file can hold a cycle, since the edit
  gate refuses one (`Graph::produces_cycle`). Refused: refusing the file.
- **The report is one status line plus the log.** After `adopt_document` (which
  calls `succeeded(Document)` and would clear an earlier line), one
  Document-family line summarises what changed, and one `tracing` warning names
  each item. The next save clears the line, since a save calls
  `succeeded(Document)`: a save accepts the repair. Refused: a new list in the
  editor, as new UI to design and build.
- **A removed node's viewer tabs close in `Document::remove_node`.** Every
  removal, by an edit or by undo or redo, goes through it. The scan of every
  tab in each frame (`Document::reconcile_with_graph`) goes. An undo of the
  removal does not open the tab again, as today.

## Load inconsistency catalogue

"Library" means the case comes from a library change between sessions or a
hand-edited file. "File" means only a hand-edited or foreign file can hold it.

| Case | Source | Now | Proposed |
|---|---|---|---|
| Undecodable RON, wrong archive shape, too large | File | Refused (`DocumentLoadError`) | Unchanged |
| Repeated node id, repeated binding port | File | Refused at decode (`graph/serde`) | Unchanged |
| Repeated subscription | File | Merged by the `BTreeSet` | Unchanged |
| Dock structure (duplicate tab, bad ratio, …) | File | Refused by palantir at decode (`DockState::validate`) | Unchanged (palantir) |
| Node with the nil id | File | Refused (`Graph::validate`) | Removed with its edges, reported |
| Func node with the nil func id | File | Refused (`Graph::validate`) | Missing-func stub, reported |
| Binding or subscription that names a missing node | File | Refused (`Graph::validate`) | Removed, reported |
| Func missing from the library | Library | Stub node in the graph; compile refuses | Detached into `Document.stubs`, reported |
| Func ports changed (signature mismatch) | Library | Stub node in the graph; compile refuses | Detached into `Document.stubs`, reported |
| Saved stub whose func resolves now | Library | (no such state today) | Attached again; edges that dangle or conflict removed, reported |
| Saved stub that still does not resolve | Library | (no such state today) | Kept in `Document.stubs` |
| Binding on an undeclared input | Library, file | Refused (`validate_declarations`) | Removed, reported |
| Wire from an undeclared output | Library, file | Refused (`validate_declarations`) | Removed, reported |
| Subscription to an undeclared event | Library, file | Refused (`validate_declarations`) | Removed, reported |
| Wire on a `const_only` input | Library, file | Loads; compile refuses (`ConstOnlyBinding`) | Removed, reported |
| Constant of a type the input does not accept | Library, file | Loads; lowers as unbound; editor locks a picker | Removed, reported |
| Picker constant not in `value_variants` | Library, file | Loads; lowers as unbound; editor shows the first variant | Removed, reported |
| Enum constant not among the registered variants | Library, file | Loads; lowers as unbound; editor shows the first variant | Removed, reported |
| `FsPath` where `FsPaths` is declared, or the reverse | Library, file | Loads; lowers as unbound; editor previews it | Removed, reported |
| `Null` on a required input | Library, file | Loads; lowers as unbound | Removed, reported |
| Wire whose producer type the input does not accept | Library, file | Loads; lowers as unbound; wire painted as warning | Removed, reported |
| Data cycle | File | Loads; each run fails `CycleDetected` | Each closing wire removed, reported |
| Self-subscription (a node on its own event) | File | Loads; the canvas refuses it, the gate does not | Removed, reported; the gate refuses it |
| Placement missing for a node or stub | File | Refused (`NodeCount`, `MissingNode`) | Placed at the origin, reported |
| Placement for no node or stub | File | Refused (`NodeCount`) | Removed, reported |
| Non-finite position | File | Refused | Moved to the origin, reported |
| Bad viewport | File | Refused | Reset, reported |
| Selected node with no placement | File | Refused | Removed from the selection, reported |
| Viewer tab on a missing node, a stub, or a non-preview node | File | Missing node refused (`MissingTab`); others load and show nothing | Tab closed, reported |
| Layout pins another tab, or another dock seed | File | Refused | Layout reset, reported |

Not inconsistencies, and unchanged: an unbound required input (a run's
missing-input verdict), a wire from a disabled producer, a `Disk` cache mode
with no disk store (documented as memory-only), numeric coercion between
`Int`, `Float` and `Bool` (the declared coercion class).

After the load, nothing the load can repair is refused, so
`DocumentValidationError` stays only for the save-time debug check.

## Wildcard machinery to remove

| Item | Where | Action |
|---|---|---|
| `OutputType` enum, `OutputType::declared`, export | `graph/func/func_output.rs`, `lib.rs` | Remove; `FuncOutput.data_type` |
| `FuncBuilder::wildcard_output` | `graph/func/func_builder.rs` | Remove |
| `FuncValidationError::InvalidWildcardInput` and its check | `graph/func/error.rs`, `func_builder.rs` | Remove |
| `DataType::or_const_type` | `data/data_type/mod.rs` | Remove; `TestGraph::returns` gets a private `literal_type` |
| `OutputTypes`, `OutputTypeSource`, the module, its tests, export | `graph/output_types/`, `lib.rs` | Remove |
| `Graph::output_source`, `input_type`, `input_spec`, `output_spec` | `graph/mod.rs` | Remove the ones nothing else reads |
| Output-kind tag byte in the signature | `graph/func/signature.rs` | Remove; `DOMAIN` v2; update the pin test |
| Wildcard arm of `declared_enum_types` | `library/mod.rs` | Read `data_type` |
| `Compiler.output_types` and its `update` per compile | `execution/compile/mod.rs` | Remove |
| `CompiledGraph.outputs` type column | `execution/compile/compiled_graph.rs` | Keep spans, drop the `DataType` per output |
| Output-type fold in the node digest | `execution/cache/runtime_cache/mod.rs` | Remove: the signature holds the same bytes (`write_type` and `write_data_type` are equal) |
| `DigestHasher::write_data_type` | `execution/cache/digest/mod.rs` | Remove if nothing else reads it |
| `NodeBuilder::output_types` test helper | `internals/program/node_builder.rs` | Remove or reduce to a count |
| `NodeSpec::passthrough`, `NodeSpec::wildcard`, `TestGraph::output_type`, `Compiled::output_types` | `internals/test_graph/` | Remove |
| `OutputTypeCache` and its tests | `darkroom/src/gui/graph_ctx/output_type_cache/` | Remove |
| `GraphCtx.output_types`, `GraphCtx::output_types`, the `GraphCtx::new` parameter | `gui/graph_ctx/mod.rs`, `internals.rs` | Remove |
| `MainWindow.output_types` and its refresh in `prepass` and `frame` | `gui/main_window/mod.rs` | Remove |
| `DockPanes.output_types` | `gui/main_window/dock_panes.rs` | Remove |
| `OutputCtx::ty` table lookup and its panic | `gui/graph_ctx/output_ctx.rs` | Read `declared.data_type` |
| `GraphCtx::port_type` output arm | `gui/graph_ctx/mod.rs` | Read the declaration |
| `Reversible::retypes_outputs`, every impl, `UndoStep::retypes_outputs`, `StepSignals.retyped` | `core/edit/step/*`, `core/open_document/mod.rs` | Replace with `changes_node_set`, true only for `NodePresence` |
| `GraphRevision` | `core/document/graph_revision.rs` | Keep for the node-cache sweep (`MainWindow::reconcile`); doc says "node set", moves only on `changes_node_set` |
| Wildcard text in docs | `compile` module doc, `typed_binding`, `Graph::validate`, `GraphIntent::commit`, `connection_ui` (`data_tint`, snap doc), `reversible.rs`, `graph_ctx` module doc, `compiled_graph.rs` | Rewrite |
| Wildcard tests | `graph/tests.rs` (passthrough chain, reroute, mismatched chains, "an output made a wildcard"), `compile/tests.rs`, `engine/tests/compile_regressions.rs`, `graph/func/tests.rs`, `internals/test_graph/tests.rs`, `gui/graph_ctx/tests.rs` (`a_wildcard_output_follows…`), `step/tests.rs` (`retypes_outputs_splits…`) | Remove or rewrite |
| `.ty.declared()` in tests | `lens/src/astro/nodes/tests.rs` | Read `data_type` |

## Stub machinery to move

| Item | Where | Action |
|---|---|---|
| `FuncResolution`, `Node::resolve`, export | `graph/node/mod.rs`, `lib.rs` | Remove |
| `Node::func(library) -> Option<&Func>` | `graph/node/mod.rs` | `-> &Func`, by id, `debug_assert!` on the signature |
| — | `library/mod.rs` | Add `Library::func_of(&Node) -> Option<&Func>` (id and signature): the one question reconcile and darkroom's reattach ask |
| `GraphValidationError::MissingFunc`, `FuncChanged`, `validate_references`' stub arms | `graph/error.rs`, `graph/mod.rs` | Remove; reconcile detaches such nodes |
| `CompileError`, `Compiler::compile`'s `Result`, export | `execution/compile/error.rs`, `mod.rs`, `lib.rs` | Remove; `compile` returns `CompiledGraph` |
| `node::internals::unresolved`, fixtures of stale nodes | `graph/node/mod.rs`, `internals/mod.rs` | Rewrite as reconcile fixtures |
| `RuntimeHost::compile`'s `Option` and its `compile failed` status | `core/runtime_host/mod.rs` | Infallible; the `false means the compile failed` returns and docs go |
| `NodeCtx.func: Option<&Func>` and every `map_or(&[][..], …)` / `is_some_and` fallback, `missing()`, `MISSING_FUNC_LABEL`, `CHANGED_FUNC_LABEL` | `gui/graph_ctx/node_ctx.rs` | A node body that is either a graph node with its `&Func` or a stub with its reason; the port accessors have no stub branch |
| `NodeCtx::resolve`'s `expect` that a placement names a graph node | `gui/graph_ctx/node_ctx.rs` | A placement names a graph node or a stub |
| Stub card border, `runnable`, `play_chip` | `node_ui/node_widget.rs`, `header.rs`, `node_menu_ui` | Read the stub body |
| `insertable_kind` ("a duplicated stub stays a stub") | `core/edit/validate.rs` | Duplicate skips stubs, so a new node always resolves |
| `NodePresence` | `core/edit/step/node_presence.rs` | Carries where the record lives (graph or stubs), so delete and undo of a stub work |
| `DocFixture::stub_at`, `stub_node` | `core/document/internals.rs` | Build a stub record |
| `OpenDocument::load` doc ("loads as a stub, its wiring kept") | `core/open_document/mod.rs` | Rewrite |

## Run-time checks of document validity to remove or assert

Each check below looks at the document itself, or at the document against the
library, after the load. With the invariant, each case is impossible, so each
becomes an assert or goes.

| Check | Where | When | Proposed |
|---|---|---|---|
| Output-type table refresh test | `MainWindow::prepass`, `frame` | Each frame | Removed with the table |
| Override reading with the type rule | `InputCtx::overridden_by`, `Graph::overridden` | Twice per input row per frame | `Graph::overridden(port, func)`: binding kind and `disabled` only |
| Mismatch test for each wire | `connection_ui::data_tint` | Each wire per frame | Remove the mismatch branch; keep the colors |
| Func lookup and signature compare | `Node::resolve` from `NodeCtx::resolve`, `NodeCtx::label`, `GraphCtx::port_type` | Many times per node per frame | Gone: a graph node resolves by construction; a stub is a separate body |
| Tab scan against the graph | `Document::reconcile_with_graph` | Each frame | Gone: `Document::remove_node` closes the tabs |
| `value_editor` fallbacks: `read_only` for a mistyped literal, `position(..).unwrap_or(0)` for a picker or enum, `as_enum().unwrap_or_default()`, `FsPath`/`FsPaths` preview mismatch | `gui/pane/graph_ui/node_ui/value_editor/mod.rs` | Each const editor per frame | `debug_assert!` or `expect`; the fallback goes |
| Viewer title fallback `"image"` for a missing node | `image_viewer::node_label` | Each viewer per frame | `expect` |
| In-flight wire `port_type(..).unwrap_or(&Any)` | `connection_ui` drag paint | Each frame of a drag | `expect` |
| `Graph::validate_with` (structure, stubs, `ConstOnlyBinding`) | `Compiler::compile` | Each compile | One `assert!` of the whole contract |
| Type gate for each input | `Compiler::typed_binding` | Each compile | Lower directly; covered by the up-front `assert!` |
| Out-of-range output | `Compiler::typed_binding` | Each compile | `debug_assert!` |
| Subscription to a dropped event skipped | compile walk, events | Each compile | `debug_assert!` |
| Cycle detection | `planner.rs` (`Error::CycleDetected`) | Each run | `debug_assert!`; the variant goes |
| `EventSeedNotFound` | `RunSchedule::collect_roots` (`schedule/mod.rs:420-425`) | Each event run | `debug_assert!`; the variant goes: fired events arrive in a batch of their own (`next_intent`'s `select!` takes one source per wake), and each install rebuilds the loop (`EventLoopTransition::for_intent`), so an event always names the installed program |
| Output-type fold | `RuntimeCache::node_digest` | Each pure node per run | Removed (see the wildcard table) |
| `Document::validate` | `io::document::save` | Each save | `debug_assert!` |
| Placement skipped when its move or raise is replayed and the node is absent | `MoveSelection::apply` (`step/move_selection.rs:65`), `Raise::apply` (`step/raise.rs:23`) | Each apply, undo, redo | `expect`: history replays steps that were valid when recorded |
| Selected node skipped when the graph does not hold it | `GraphIntent::Duplicate` (`graph_intent/mod.rs:229`) | Each duplicate | `expect` for a graph node; a stub is skipped by rule |
| Placement skipped when its node is absent | node paint loop (`node_ui/mod.rs:138`) | Each node per frame | `expect`: the order comes from the same context in the same pass |

## Run-time checks that already assert the contract

These stay. Their contract moves from "the compile lowered it" to "the load and
the gate admitted it", and each message says so where it names the compiler.

- `DynamicValue::required_*` panics on a misread input
  (`scenarium/src/data/dynamic_value.rs`).
- `Preset::resolve`'s `expect` on the pick (`lens/src/astro/config/preset/mod.rs:72`).
- `NodePresence`'s `expect` that the view places every node.
- `assert_reported` in `RunState` (a report names a node of the acknowledged
  program).
- `ExecutionEngine::validate_debug`, `validate::validate_debug`,
  `RunSchedule::validate_debug` — already debug-only.

## Checks that stay, with the reason

These look at state that can correctly lag the document. They are not checks
of the document's validity.

- **Gestures held across frames.** A node can go during a drag, for example by
  an undo: `connection_ui` (`contains` at lines 86, 117, 135),
  `subscription_ui` (94, 107), `group_drag` (102), `preview_drag` (60), the
  `MoveSelection` intent's skip (`graph_intent/mod.rs:371`), the `Raise`
  intent (450), the `SetSelection` filter (444), and the edit gate's
  `live_node`, which turns a stale intent into no step.
- **Caches keyed by node.** `GraphUI::retain_nodes`,
  `MainWindow::reconcile`'s viewer-state retain, `PreviewStore`'s sweep
  (`holds_preview_node`). The inspector panel loop skips a node that is gone
  (`paint/inspectors.rs:151`): the sweep runs in `App::update`, but `record`
  applies undo and redo before the draw, so a node can go in between.
- **Requests merged with a later install.** `recv_many` merges every pending
  message into one batch, and the last `Update` wins. A `RunNodes`, `EvictCache`
  or `FlushCache` for a node can thus arrive with a program that no longer
  holds the node: `NodeSeedNotFound` (`schedule/mod.rs:410`) and the
  `filter_map` in `RuntimeCache::evict` and `flush` (`runtime_cache/mod.rs:200,
  710`). This is the worker protocol, not the document (see `.notes/ISSUES.md`).
- **Commands raised before a drain.** `RuntimeHost::run_node`'s
  `compiled.contains(node_id)` (`runtime_host/mod.rs:225`): an `AppCommand` runs
  after its frame's drains, so a node that an edit in the same frame removed is
  gone.
- **Results of an earlier run.** The `RunState` preview gate
  (`run_state/mod.rs:421`) drops a preview from a program that is no longer
  installed.
- **Data from disk.** The disk cache format (`disk_store/format`) reads
  untrusted bytes and returns `Result`.
- **New edits.** `Graph::produces_cycle`, `accepts_wire` and the edit gate check
  an edit, not the document.

## Progress

- Batches 1 to 3 are done; batch 4 is open. `cargo drive check` and `cargo
  drive test` pass.
- Batch 1 also took the darkroom half of the wildcard removal, so the
  workspace builds: `OutputTypeCache` and its plumbing are gone, `OutputCtx::ty`
  and `GraphCtx::port_type` read the declaration, and `retypes_outputs` is
  `changes_node_set`. `FuncBuilder` refuses an `Any` output. The cache digest
  domain is `scenarium-cache-v8`.
- Batches 2 and 3 landed together, because batch 2 alone leaves darkroom
  holding stub nodes in a graph that the compile asserts holds none.
- `Library::func_of` returns `Result<&Func, Unresolved>`, not `Option`, so the
  reconcile and the stub card read the reason from one call. A stub's reason is
  asked of the library each frame rather than stored: the library is fixed per
  process, so the answer cannot change.
- `Graph::admit_onto` admits a binding onto a node an insertion has not added
  yet; `Graph::admit` finds the consumer and calls it.
- The load decodes only (`io::document::load`); `Document::reconcile` makes the
  result consistent, and `OpenDocument::load` returns `Loaded { open, report }`.
  `StatusLog::warn` parks the summary line in the sticky slot.
- A viewer tab is held only by a preview node in the graph; a stub has none.
- `port_color::data` lost its `PortKind` parameter, and the palette's
  `output_port` color is gone: no output is `Any`.
- `Graph::overridden(port, func)` already read only the binding kind and
  `disabled`; item 9 needed no change.
- The canvas snap filter asks `Graph::admit`, so the canvas and the edit
  gate hold a wire to one rule; `GraphCtx::port_type` cannot fail;
  `DataType::compatible_with` is crate-private.
- `GraphValidationError::NilFuncId` is gone: a nil func id is a node that does
  not resolve, which `func_of` answers.
- The edit gate checks a new id against the graph and the stubs. A stub's
  title is read-only (`InlineRename::read_only`), and its context menu offers
  only Remove.
- Both reconciles assert in debug builds that their result validates.
- A status line carries a `Severity`, so the load summary paints in the
  warning color, not the error color.
- `AGENTS.md` has the new convention.

## Implementation plan

### Batch 1 — scenarium: remove wildcards

1. [x] `FuncOutput.data_type`; remove `OutputType` and its export,
   `wildcard_output`, `InvalidWildcardInput`, `or_const_type`. Write
   `literal_type` in `internals/test_graph`. Update every reader, including
   `lens/src/astro/nodes/tests.rs`.
2. [x] Remove `graph/output_types/`, its export, `Graph::output_source` and the
   `input_type` / `input_spec` / `output_spec` helpers nothing else reads.
   Remove `Compiler.output_types`.
3. [x] `CompiledGraph.outputs` keeps the spans without a `DataType` per output.
   Remove the output-type fold from `RuntimeCache::node_digest` and
   `DigestHasher::write_data_type`. Reduce `NodeBuilder::output_types`.
4. [x] `FuncSignature::of`: name and `write_type` per output, no tag byte;
   `DOMAIN` v2; update `a_small_func_has_the_pinned_signature` and remove the
   wildcard case of `a_signature_covers_the_ports_and_nothing_else`.
5. [x] `declared_enum_types` reads `data_type`. Remove the wildcard tests and
   the `TestGraph` helpers listed above. Rewrite the docs listed above.

### Batch 2 — scenarium: resolved-only graph, admission, reconcile, asserts

6. [x] `Library::func_of(&Node) -> Option<&Func>` (id and signature).
   `Node::func(library) -> &Func`. Remove `FuncResolution`, `Node::resolve`,
   `MissingFunc`, `FuncChanged` and the stub arms of `validate_references`.
7. [x] `BindingRejection` in `graph/error.rs`: `UndeclaredInput`,
   `UndeclaredOutput`, `WiredConstOnly`, `ConstMismatch`, `WireMismatch`. A
   `SubscriptionRejection`: `UndeclaredEvent`, `SelfSubscription`.
   `Graph::admit(library, port, binding)` and
   `Graph::admit_subscription(library, subscription)`, over resolved nodes
   only. The `Const` arm is `FuncInput::accepts_const` with the enum-variant
   lookup from `binding_satisfies`; the `Bind` arm reads the producer's
   declared output. Remove `Library::binding_satisfies`.
8. [x] `Graph::reconcile(&mut self, library) -> Reconciled`, in order: remove
   nil-id nodes and edges that name a missing node; detach each node that
   `func_of` does not resolve (the nil func id included) and return its
   `DetachedNode`; remove each binding and subscription that `admit` refuses;
   remove each wire that closes a cycle, walking in sorted id order.
   `Reconciled` lists each removal with its reason and holds the detached
   records.
9. [x] `Graph::overridden(port, func)`: binding kind and `disabled` only.
   Revert `InputCtx::overridden_by` and the override test closure; remove the
   mistyped cases from both override tests.
10. [x] Compile: `compile` returns `CompiledGraph`; remove `CompileError`. One
    `assert!` at its start checks the contract: structure (`Graph::validate`),
    each node resolves, each binding and subscription admitted, no cycle.
    `typed_binding` and the event walk lower directly, both behind
    `debug_assert!`.
11. [x] Planner and worker: `CycleDetected` and `EventSeedNotFound` become
    `debug_assert!` and the variants go. `NodeSeedNotFound` and the evict and
    flush skips stay (see "Checks that stay").
12. [x] Tests: `reconcile` removes exactly each rejection kind, removes a nil-id
    node, detaches a missing func, a changed func and a nil func id, keeps
    `Null` on an optional input, and removes one closing wire of a cycle; a
    reconciled graph compiles. Rewrite the drift and stub tests in
    `compile/tests.rs`, `engine/tests/compile_regressions.rs`,
    `graph/tests.rs` and `internals/` as reconcile tests.

### Batch 3 — darkroom

13. [x] Stubs: a `Stubs` collection of `DetachedNode` records keyed by node id,
    with a reason (missing func, changed ports) computed at load and not
    serialized; `Document.stubs`. Placements cover graph nodes and stubs; ids
    are disjoint. `Document::validate` checks both.
14. [x] Load, in order: decode; repair the view and layout; attach each saved
    stub that `func_of` now resolves, after removing its edges that conflict
    with a bound input; `graph.reconcile`; add the detached records to
    `Stubs`; place any node or stub with no placement; set `dirty` when
    something changed. `open_at_launch`, `load_preferred` and
    `App::load_document` write one Document-family status line after
    `adopt_document`, and one `tracing` warning for each item. Remove
    `validate_declarations`, the refusals that became repairs, and
    `core/document/declaration.rs`.
15. [x] Edit gate: `SetInput` and `seed_bindings` call `graph.admit`;
    `SetSubscription` calls `graph.admit_subscription`. A refusal is
    `MalformedIntent::Rejected { port, rejection }` or the subscription
    variant. Remove `declared_input`, `declared_output`, `declared_event`,
    `wirable` and the variants they raise. Keep the presence and cycle checks.
    Duplicate skips stubs; `insertable_kind` requires a resolving node.
16. [x] Undo steps: `NodePresence` records whether its node lives in the graph
    or in `Stubs`, so a stub can be deleted and the delete undone.
    `retypes_outputs` becomes `changes_node_set` (true only for
    `NodePresence`); `StepSignals` and `GraphRevision` follow.
    `Document::remove_node` closes the node's viewer tabs;
    `Document::reconcile_with_graph` goes.
17. [x] Editor: `NodeCtx` holds a body that is a graph node with its `&Func` or
    a stub with its reason; the port accessors lose their `Option` fallbacks;
    the stub card reads the stub body. Remove the `data_tint` mismatch branch;
    turn the `value_editor`, `node_label`, `data_tint` and drag-paint
    fallbacks into `expect` or `debug_assert!`. No output then reads `Any`,
    so `port_color::data` loses its `PortKind` parameter and
    `theme.ports.output` goes.
18. [x] `RuntimeHost::compile` becomes infallible; its `compile failed` status
    and the `false means the compile failed` returns go.
19. [x] Other darkroom checks become asserts: `Document::validate` in
    `io::document::save`; the placement skips in `MoveSelection::apply` and
    `Raise::apply`; the selected-node skip in `GraphIntent::Duplicate`; the
    node paint loop's skip. Update the messages of the asserts in "Run-time
    checks that already assert the contract" that name the compiler.
20. [x] Tests: a load of a document with each case in the catalogue removes,
    repairs, detaches or attaches it, reports it and marks the document dirty;
    a stub that resolves again drops an edge that conflicts; a graph with a
    stub runs the rest; the edit gate refuses a mistyped constant, a mistyped
    wire and a self-subscription; deleting a stub undoes; removing a node
    closes its viewer tab; the node sweep moves only on a node-set change.

### Batch 4 — verify and migrate

21. [ ] `cargo drive check`, then `cargo drive test`.
22. [ ] One-off, outside the repo: restamp `~/Documents/lumos-test.darkroom` to
    v2, only for nodes whose v1 signature matches the current ports. Load it
    and run it.
23. [ ] Fold each lasting decision into the module docs of `scenarium::graph`,
    `scenarium::execution::compile`, `darkroom::core::document` and
    `darkroom::core::open_document`, and delete this proposal. Remove "audit
    overridden_by" from `.notes/TODO.md`.
