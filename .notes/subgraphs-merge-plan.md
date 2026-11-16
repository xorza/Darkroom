# Subgraphs: the merge as reviewable commits

`subgraphs` is one commit, `0b9370799 Add subgraphs`, over `4eb4ad93d`: 334 files,
+21 976 / −6 562. This plan replays it onto `master` as 12 commits: renames and moves first, then
the shape changes that keep behavior, then the features. Temporary: deleted once the series lands.

## Progress

| # | Commit | State |
|---|---|---|
| 1 | Rename the port declarations `InputDecl` and `OutputDecl` | `78adc05e6`, pushed |
| 2 | Name program nodes by `ProgramId` | `a718d77fe`, pushed |
| 3 | Keep stubs in scenarium's `Graph` | `52c03147e`, pushed |
| 4 | Make `DataType` a small value of ids | `9ddb3e708`, pushed |
| 5 | Name ports by id | `6c01623b8`, pushed |
| 6 | Subgraphs in scenarium, compiled flat | `09596151d`, pushed |
| 7 | The editor holds every graph of a document | `69549dbfb`, pushed |
| 8 | The window shows a pane per graph | `21b98e664`, pushed |
| 9 | Edit subgraphs | `032799367`, pushed |
| 10 | Move node sets | `fc633c202`, pushed |
| 11 | Share subgraphs | in the worktree |
| 12 | | to do |

## Rules for the series

- **Each commit builds and passes the gate** (`cargo fmt --all && … && cargo gate-test`), so the
  series bisects. The cost: commits 2, 3, 6 and 7 hold a few lines of interim code the tip does
  not have, named under each commit.
- **The last commit's tree is the tip's tree**, apart from the lines under Differences from the
  tip. `git diff 0b9370799 <last>` shows only those, which proves nothing else was lost or added
  in the split.
- **A review fix to a line the tip also holds is carried forward**: each later commit keeps it
  rather than taking the tip's line back, and it is listed under Differences from the tip.
- **A file belongs to the commit that writes its final form.** A file that spans several is staged
  by hunk; the lists below mark it *partial*.
- **The two fixture documents are rewritten in every commit that changes the file shape** (2, 3,
  4, 5, 6, 7): `darkroom/sample/sample.darkroom` and `darkroom/src/gui/app/session/bench.darkroom`, each
  by a load and a save in that commit's build, never by hand.
- Sizes are `+/−` of the files each commit owns outright, so they are approximate.

## Renames and moves

| Before | After | Commit |
|---|---|---|
| `graph/func/func_input.rs` `FuncInput` | `graph/ports.rs` `InputDecl` | 1 |
| `graph/func/func_output.rs` `FuncOutput` | `graph/ports.rs` `OutputDecl` | 1 |
| `Func { inputs, outputs }`, `Func::default_bindings` | `Func { ports: Ports }`, `Ports::default_bindings` | 1 |
| `exec/compile/tests.rs` | `exec/compile/tests/mod.rs` | 1 |
| `NodeId` on the execution side (reports, seeds, caches, worker messages, `ctx.cur_node_id()`) | `ProgramId` (`ctx.cur_program_id()`) | 2 |
| `EventPort` in `RunSeeds` and `FireEvents` | `ProgramEvent` | 2 |
| `TabRef::ImageViewer(NodeId)`, `PreviewSink`/`PreviewStore` keys | `ProgramId` | 2 |
| `darkroom/src/core/document/stubs.rs` `Stubs` | `scenarium/src/graph/stub.rs` `Stub`, held in `Graph` | 3 |
| `Reconciled.detached: Vec<Detachment>` | `Repair::Stubbed { reason: DetachReason }` | 3 |
| `Node.cache` field | `NodeKind::Body { cache }`, `Node::cache()`/`cache_mut()` | 6 |
| `library/type_entry.rs` `TypeEntry` | `data/data_type/enum_type.rs` `EnumType`, `custom_type.rs` `CustomType` | 4 |
| `TypeId` | `EnumId`, `CustomTypeId`, `FileKindId` | 4 |
| `DataType::FsPath(Arc<FsPathConfig>)`, `FsPathMode` | `DataType::Dir`, `DataType::File { kind, mode }`, `FileMode`, `FileKind` | 4 |
| `Library::register_type`, `type_entry`, `enum_variants`, `types()` | `register_enum`/`register_custom_type`/`register_file_kind`, `enum_type`/…, `type_count()`, `data_types()` | 4 |
| `InputPort.port_idx`, `OutputPort.port_idx`, `EventPort.event_idx` | `port: PortId`, `event: PortId` | 5 |
| `NodeKind::Func(FuncRef)` | `BodySource::Func(FuncId)`; `FuncRef` stays on the compiled side only | 5 |
| `PortRef.port_idx` | `PortRef.port` | 5 |
| `Node::func(&Library)`, `Library::func_of` | `Node::decl(Scope)`, `Scope::resolve` | 6 |
| `Graph::reconcile(&Library)` | `GraphSet::reconcile` | 6 |
| `Document { graph, stubs }` | `Document { graphs: GraphSet, main_view, sides }` | 8 |
| `DocumentRequest::Graph(intent)` | `DocumentRequest::Graph { graph, intent }` | 8 |
| `TabRef::Graph` | `TabRef::Graph(GraphId)` | 9 |
| `InspectTarget::Node(NodeId)` | `InspectTarget::Node { addr: NodeAddr, through }` | 9 |
| `GraphCtx::new(app, open, display)` | `DocCtx::new(app, open)`, then `GraphCtx::of(doc, graph, display)` | 9 |

## Phase 1: renames and moves

### 1. Rename the port declarations `InputDecl` and `OutputDecl` — done

### 2. Name program nodes by `ProgramId`

The execution side names a node by `ProgramId`, a path of instances held inline down to the
authored node, beside `InstancePath`; every path is a bare node id until commit 6. A disk blob is
named by the digest of its path and holds the path, which a read checks. Reports, seeds,
evictions, the worker protocol, the run state, the preview sink and store, and the image viewer
tabs carry it.

- Owns: `scenarium/src/graph/{program_id,instance_path}.rs`, `exec/program_event.rs`, `exec/`
  apart from `compile/mod.rs` and `compile/tests/`, `worker/`, `runtime/`, `internals/{engine,
  worker}.rs`, `internals/program/`, `internals/test_graph/compiled.rs`, `clippy.toml`
  (`large-error-threshold`), `darkroom/src/core/{preview_sink,worker_bridge}/`,
  `darkroom/src/gui/state/preview_store/`, `gui/pane/image_viewer/`,
  `gui/state/run_state/run_outcome.rs`, `gui/app/session/bench.rs`.
- Partial: `exec/compile/mod.rs` (the program keyed by `ProgramId`, no flatten),
  `gui/state/run_state/mod.rs` (`nodes` by `ProgramId`, no `instances`),
  `core/document/tab_ref.rs` (the `ImageViewer` arm).
- About +1 600 / −750, mostly mechanical. Review `program_id.rs`, `instance_path.rs` and
  `disk_store/format`.
- **Interim code:** `GraphCtx::program_id(node_id)` answers the main graph's id alone, and
  `RunCommand` and the worker bridge's eviction and flush still name one node, which `App::run`
  turns into its `ProgramId`; commits 7 and 8 widen them to every instance of a node.
- Held back to commit 6: `CompiledGraph::persists_to_disk`, whose one caller is the runtime host
  compiling a `GraphSet`.
- The bench fixture's viewer tabs are rewritten here, as a `ProgramId` is written as its list.

### 3. Keep stubs in scenarium's `Graph`

A node the graph cannot hold becomes a `Stub` in the `Graph` beside its nodes, with its
`DetachReason`; one reconcile attaches each stub that resolves again and detaches each node that
does not. Darkroom's `Stubs` and its attach rules go.
This replaces "Stubs are darkroom's, not scenarium's" in the `document` module doc.

- Owns: `scenarium/src/graph/{stub,detached}.rs`, `graph/serde/`,
  `darkroom/src/core/document/stubs.rs` (deleted), `core/document/graph_view.rs`,
  `core/edit/validate.rs`, `core/edit/step/set_node.rs`.
- Partial: `graph/mod.rs` (the stub table, `resolving_stubs`, `reattach`, `unresolved`),
  `graph/reconciled.rs` (no `GraphSetReconciled` or `BoundaryAdded`), `graph/error.rs`
  (`DetachReason::Unresolved` alone), `core/document/{mod,load_report}.rs`.
- **Interim code:** `Graph::reconcile(&Library)` as a single-graph entry point, its steps
  private and read against the library, which commit 6 replaces with `GraphSet::reconcile` over
  a `Scope`.
- Moved to commit 6: `Node.cache` into `NodeKind::Body`, as its only reason is a kind of node
  with no cache, which commit 6 brings.
- About +500 / −450.

## Phase 2: shape changes that keep behavior

### 4. Make `DataType` a small value of ids, and register file kinds

`DataType` becomes `Copy` and holds ids alone: `Dir`, `File { kind, mode }`, `Custom(CustomTypeId)`,
`Enum(EnumId)`. The library keeps one table per kind of type, and `Library::check` holds once that
every type a func names is registered. File ports feed by kind subset. The file dialogs read the
extension list from the registry (`gui/path_type.rs`); port colors and value formatting follow.
This removes the `Arc<FsPathConfig>` the document serialized.

- Owns: `scenarium/src/data/data_type/` (all), `library/` (all, `type_entry.rs` deleted),
  `data/{dynamic_value,codec}`, `exec/cache/{runtime_slot,disk_store/error,
  disk_store/store_outcome}.rs`, `exec/engine/tests/{mid_run_release,resource_binds,
  cache_persistence/cache_modes}.rs`, `internals/blob.rs`, `lens/src/image/{format,codec}`,
  `lens/src/astro/{masters,nodes/mod,nodes/tests}.rs`, `darkroom/src/gui/path_type.rs`,
  `gui/widgets/format/`, `gui/theme/`, `gui/pane/graph_ui/node_ui/const_editor/`,
  `core/runtime_library.rs`.
- Partial: the `lens` node files, `config_node.rs`, `fs_watch`, `port_row`, `input_ctx`.
- Held back to commit 6: unknown types — the load report's `UnknownType`, a constant of an
  unregistered enum kept unchecked (`const_satisfies` refuses it here, as `is_some_and`), the
  const editor's read-only field for a type the library lacks and its test, and the neutral port
  color through `GraphCtx::port_color`. Only an interface port can name a type the library lacks.
  `Library::knows` lands here, as `Library::check` reads it.
- Held back to commit 9: `Library::data_types`, whose one caller is the interface card's retype.
- The signature stays v2 and writes a directory and a file of any kind and mode as the one kind a
  path was, so no saved node turns into a "changed ports" stub; commit 5's v3 writes them apart.
- `AppCommand` and `EditCommand` become `Copy`, as a `PathPick` is; the commit that brings
  `AppCommand::Warn(String)` takes `Copy` off `AppCommand` again.
- About +1 950 / −750.

### 5. Name ports by id

Every port, input, output and event, carries a `PortId`. A func numbers its ports, and a `lens`
config field's number is a pinned hash of its Rust name. Bindings and subscriptions name ports by
id; a node names its func by `FuncId` alone. `Unresolved::ChangedPorts` goes, with the stub it
made. The compiled program stays positional: the compile maps each id to its position once, and
the signature is v3. The editor's `PortRef`, widget ids and gestures key by port id. `AGENTS.md`
gains the port-id rule.

- Owns: `scenarium/src/graph/{identity,binding}.rs` (the port half), `graph/func/{signature,
  error,func_builder}.rs`, `builtin/*`, `internals/test_graph/tests.rs`, `lens/` (the numbers),
  `darkroom/src/gui/{port_ref,node_key}.rs`, `core/edit/unwired.rs`, the gesture tests
  (`wire_ui`, `tie_ui`, `breaker_ui`, `group_drag`, `preview_drag`), `canvas_geometry` and
  `tie_layout` tests, `glyph_drag.rs`, `output_ctx.rs`, `port_row/`, both fixture documents,
  `AGENTS.md`.
- Partial: `graph/ports.rs` (ids, `numbered_port`, `PortsError`), `graph/mod.rs`,
  `graph/tests.rs`, `exec/compile/mod.rs`, `core/edit/graph_intent/tests.rs`.
- The fixtures are the tip's, taken back to one graph: every binding names its port by id and a
  node names its func by id alone.
- `TestGraph::evolve_func` goes, as a node no longer holds its func's signature; a test edits a
  func with `edit_func` and numbers a port it adds.
- About +1 250 / −830, mostly mechanical.

## Phase 3: features

### 6. Subgraphs in scenarium, compiled flat

`Subgraph`, an immutable `Interface`, the `Inputs`/`Outputs` boundary nodes, `NodeKind::Instance`,
`NodeDecl` and `Scope`, so every node resolves to a declaration and `Graph::admit` stays the one
admission rule. `GraphSet` holds the main graph and the subgraphs with every rule across them:
scope, uses, recursion, the depth and size bounds (`admit_nodes`), validation, the load's
reconcile (detach, attach, loops, bounds, missing boundary nodes), unknown types, the digest.
`GraphId`, `NodeAddr` and `SubgraphId` join the identity module. `Compiler::compile(&GraphSet,
&Library)` expands each instance by `InstancePath`; a binding that reaches a boundary or an
instance is lowered to a func's output, a constant or nothing; a const-only `Inputs` port lowers
to its constant; a disabled instance disables every node inside. Plan and execute see no
hierarchy. The editor's `Document` holds the `GraphSet` and edits, shows and runs its main graph;
the load reports a repair inside a subgraph under that subgraph, checks and maps the paths of
every graph, and reports each type an interface names that the library does not register
(`UnknownType`). Such a type's constant shows read-only and its port in the neutral color
(`GraphCtx::port_color`).

- Owns: `scenarium/src/graph/{graph_set/,interface,node_decl,scope,subgraph}.rs`,
  `graph/node/mod.rs`, `graph/error.rs`, `graph/reconciled.rs`, `graph/mod.rs`, `graph/tests.rs`,
  `exec/compile/mod.rs`, `compile/tests/{mod,flatten}.rs`, `internals/test_graph/{mod,sets}.rs`,
  `internals/mod.rs`, `lib.rs`, both fixture documents.
- Held back to commit 9: `interface_change.rs` with `plan_interface`/`write_interface` and
  `GraphSet::set_interface`, whose only caller is the editor's interface edit, and
  `Library::data_types`, whose only caller is the interface card.
- Model and flatten are one commit: the tip's compile takes a `GraphSet`, and an interim compile
  of one `Graph` would be code no later commit keeps.
- **Interim code:** `GraphCtx::scope`, the driver and the edit gate resolve against
  `GraphId::Main`; `Document` keeps one `main_view`, and the fixtures hold no `sides` or `active`.
  An add past a placement bound (`GraphSet::admit_nodes`) commits nothing, as `Ok(None)`, until
  commit 9's `EditRefusal::Placement` names it on the status bar.
- About +3 800 / −480 in scenarium, of which `graph_set/tests.rs` is 1 000; +1 300 / −980 in
  darkroom, most of it the two fixtures.

### 7. The editor holds every graph of a document

`Document { graphs, main_view, sides }`, each `SubgraphSide` holding its view alone here;
`GraphSlot` borrows one graph and its view apart. Intents and undo steps name their graph
(`DocumentRequest::Graph { graph, intent }`, `UndoStep::Graph { graph, step }` over a
`GraphStep`), and `EditFacts.touched` says which graphs an edit wrote, which moves each graph's
own revision. The load gives each subgraph a side, drops a side of none and repairs every view
(`DocumentRepair::View { graph, repair }`). An eviction or a flush reaches every instance of the
authored node (`NodeAddr`). Inside a subgraph an edit passes over its boundary nodes.

- Owns: `darkroom/src/core/document/{mod,error,graph_revision,subgraph_side,internals,tests}.rs`,
  `core/edit/{document_queue,document_request,edit_facts,validate}.rs`, `core/edit/step/` apart
  from the subgraph steps, `core/edit/graph_intent/`, `core/edit/undo_history/tests.rs`,
  `core/open_document/`, `core/runtime_host/`, `core/worker_bridge/`, both fixture documents.
- Partial: `core/document/load_report.rs` (no `ThroughCleared`, `ActiveReset` or `TabClosed`).
- Held back to commit 8: `internals/fixture.rs`'s `nested_preview`, whose callers are the MCP
  tests of a preview inside an instance.
- **Interim code:** the GUI's `Requests` queues every edit on `GraphId::Main`, and the duplicate
  sites, `Session::apply_edit`, the window's sweep and `App::run` name the main graph, until
  commit 8 gives each its pane's graph. A removal of a boundary node commits nothing until commit
  9's `EditRefusal::BoundaryNode`. The load names a closed tab as `ViewerClosed` or
  `InspectorClosed` until commit 8's `TabClosed`. The fixture's `insert_subgraph` writes the set
  and the side itself until commit 9's `Document::insert_subgraph`.
- About +1 060 / −590.

### 8. The window shows a pane per graph

`DocCtx` is what the surround reads and `GraphCtx` narrows it to one graph. `MainWindow` keeps a
`GraphUi` per open graph tab (`TabRef::Graph(GraphId)`). The window has one active graph, set by a
picked tab or a pane's input; menus, status bar and inspector act on it. `Enter` navigates into a
subgraph through an instance. A pane reads each run result through the instance it shows (a
side's `through`), and a pinned inspector keeps its own. A run names its program node and an
eviction its authored node (`RunCommand::Node(ProgramId)`, `EvictCache(NodeAddr)`), and
`ViewCommand::Frame` names its graph. The load clears a stale `through`, resets an active graph
the set lacks and closes each stale tab (`TabClosed`). The driver's targets and the MCP server
reach any graph's pane through its tab.

- Owns: `darkroom/src/gui/frame/{doc_ctx,requests/,app_command}.rs`, `gui/graph_ctx/`,
  `gui/main_window/{mod,dock_panes,status_bar/}`, `gui/app/session/`,
  `gui/pane/graph_ui/{mod,internals,canvas,canvas_bg,toolbar,wid}.rs`,
  `gui/pane/graph_ui/gesture/` apart from the palette's subgraph rows, the boundary port and the
  clipboard, `gui/pane/graph_ui/node_ui/`, `gui/pane/inspector.rs`, `gui/pane/image_viewer/`,
  `core/document/{tab_ref,inspect_target,layout_file,subgraph_side,error}.rs`, `driver/`,
  `app/src/mcp/`, `internals/fixture.rs`, both fixture documents.
- Partial: `core/document/{mod,load_report,tests}.rs`, `core/open_document/mod.rs`,
  `core/edit/document_{request,queue}.rs`, `gui/app/mod.rs`, `gui/main_window/menu_bar.rs`.
- Held back to commit 9: the pane header with its path, use count and Make Unique; an instance's
  rolled-up status and the footer's culprit (`InstanceRunState`, `RunView::Instance`,
  `failed_inside`), and the driver test's check of it; an undo bringing its graph forward;
  `Requests::land` and `AppCommand::Warn`.
- About +2 250 / −1 150, mostly the context split.

### 9. Edit subgraphs

`SubgraphIntent`: add, remove and set interface, with steps `SetSubgraph` and `SetInterface` in
the same undo entry as their batch's graph edits; Make Unique and Remove Unused build on them.
`InterfaceChange` is scenarium's reversible record and drops wires in every graph. A wire dropped
on a boundary node becomes a port (`port_edit.rs`); the inspector's interface card orders,
renames, retypes and removes ports. The pane header shows the instance path, the use count and
Make Unique; the palette lists the document's subgraphs. An instance rolls up the worst status
and summed time inside, and its footer names the node that failed. A refused edit names why on
the status bar (`EditRefusal`). An undo brings its graph to the front, or names the subgraph it
changed when no tab shows it.

- Owns: `scenarium/src/graph/interface_change.rs`, `graph_set/{mod,tests}.rs`, `library/mod.rs`,
  `lib.rs`; `darkroom/src/core/edit/{subgraph_intent/,port_edit,make_unique,error,edit_facts}.rs`,
  `core/edit/step/` apart from `set_side` and `set_definition`, `core/edit/graph_intent/`,
  `gui/pane/{interface_card,inspector}.rs`, `gui/pane/graph_ui/pane_header.rs` (without the
  library row), `gesture/new_node_ui/` (without the library rows), `gesture/wire_ui/`,
  `node_ui/node_footer.rs`, `gui/state/run_state/`, `gui/graph_ctx/node_ctx.rs`.
- Partial: `core/document/mod.rs` (no `replace_subgraph`), `core/open_document/mod.rs`,
  `gui/app/mod.rs`, `gui/main_window/menu_bar.rs` (Make Unique and Remove Unused alone).
- Held back to commit 11: `SubgraphIntent::{SetShared, Update, Synced}` with `SetSide` and
  `SetDefinition`, `Document::replace_subgraph`, `OpenDocument::content_revision`, the pane
  header's library row and its state, the interface card's Shared box, the palette's library
  rows.
- **Interim code:** the pane header is a free function, as nothing of it outlives a frame until
  commit 11 caches the library standing; the palette's pick is one `GraphIntent`. The subgraph
  intent's doc names Make Unique's copy until commit 10 brings the collapse the tip names.
- About +3 250 / −230, of which `graph_set/tests.rs`, the interface card, `port_edit.rs` and the
  pane header are 1 200.

### 10. Move node sets: Duplicate, Copy, Paste, Collapse, Expand

One `Fragment` captures nodes with the wires among them, their places and ties, remaps ids and
lands them in a graph; Duplicate is a capture and a paste. A `Clip` is a fragment plus the
subgraphs it uses, RON under a header line on the system clipboard. Collapse makes a subgraph and
one instance of it, refused when a wire leaves the selection and another returns; Expand
reverses it. Bringing a definition in (`bring_in.rs`) reuses an equal one by digest, takes a
different one in beside it with a fresh id, and strips its output paths.

- Owns: `darkroom/src/core/edit/{fragment,bring_in}.rs`, `core/edit/collapse/`, `core/clip/`,
  `core/mod.rs`, `gui/frame/{clip_commands,shortcuts}.rs`, `gesture/shortcuts.rs`,
  `core/edit/graph_intent/mod.rs`, `gui/app/session/tests.rs`.
- Partial: `gui/main_window/menu_bar.rs` (all but Export).
- Held back to commit 11: `Carried::synced`, which a place from the library carries, and
  `BroughtIn::enter`, whose one caller is an update.
- **Interim code:** `bring_in.rs`'s module doc names a paste alone, and its first rule says
  nothing brought in changes a held definition, where the tip names Update.
- About +1 700 / −190.

### 11. Share subgraphs: the library folder, Publish and Update

A library folder, set in Preferences, watched through `notify` and scanned into a
`SharedLibrary`; the palette lists its shared subgraphs. A side's `shared` and `synced` read sync
state: in sync, update available, changed here, changed in both. Update from Library is an
undoable edit; Publish to Library arms, then confirms, in the pane header. `darkroom` takes
`notify`.

- Owns: `darkroom/src/core/shared_library/`, `core/io/preferences/`, `core/edit/step/set_side.rs`,
  `core/status_log/mod.rs`, `gui/frame/app_ctx.rs`, `gui/pane/preferences.rs`, the rest of
  `pane_header.rs` and `palette_rows/`.
- About +1 000 / −80.

### 12. Export with resources

Export writes a document, or a `.darkroomz` bundling the resources the export dialog picks under
`resources/<digest>/<name>`, which unpack checked and read-only beside the archive.
`darkroom/AGENTS.md` gains the archive's line; `darkroom` takes `blake3`.

- Owns: `darkroom/src/core/io/{export,mod}.rs`, `core/io/document/`, `gui/app/{mod,export_dialog}.rs`,
  `gui/dialogs/mod.rs`, the rest of `menu_bar.rs`, `darkroom/Cargo.toml`, `Cargo.lock`,
  `darkroom/AGENTS.md`, `.notes/{subgraphs,TODO}.md`.
- About +500 / −40.

## Settled

- **`.notes/subgraphs.md` lands as the branch wrote it**, in commit 12, and goes in a later change.
- **serde's `rc` feature is dropped.** At the tip, `cargo check -p scenarium -p lens -p darkroom
  --all-targets` passes without it, in a build with no `rc` at all (`cargo tree -e features -i
  serde -p darkroom` shows none; in the workspace, `rmcp` turns it on for `darkroom-app`). No
  commit gains `rc`, and the issue entry does not land.
- **`TODO.md`** loses "subgraphs" and gains "ghost wires for stubs" in commit 12.

## Differences from the tip

What the series' last tree holds where the tip holds otherwise.

- **`scenarium/Cargo.toml`, `.notes/ISSUES.md`:** no serde `rc`, and no issue entry for it.
- **`scenarium/src/exec/compile/mod.rs`:** the closure that appends a node's inputs names each
  `input`, not `func_input` (commit 1).
- **`scenarium/src/graph/ports.rs`:** `Ports::default_bindings` returns
  `impl Iterator<Item = BindingEntry>` without `+ '_`, which edition 2024 captures anyway
  (commit 1).
- **`scenarium/src/builtin/{system_library,worker_events_library/mod}.rs`:** `InputDecl` and
  `OutputDecl` come in one `use` (commit 1).
- **`scenarium/src/data/dynamic_value.rs`:** `required_fs_path` and `required_fs_paths` name a file
  taken one or several at a time, the tip's `FileMode` (commit 4).
- **`scenarium/src/exec/cache/digest/mod.rs`:** the module doc lists a directory or a file as the
  types a reference is dereferenced through, with no "path" the tip no longer has (commit 4).
- **`scenarium/src/graph/mod.rs`:** the module doc lists `ports` among the modules beside the
  graph (commit 1); commit 6 keeps it when it rewrites that paragraph.
- **`darkroom/src/gui/state/run_state/tests.rs`:** the helper that makes a test's program id is
  `program_id(n)`, not `node_id(n)` (commit 2); commit 9 keeps the name.
- **`darkroom/src/core/document/tests.rs`:** a test that the load closes a viewer whose path
  goes through an instance (commit 2).
- **`scenarium/src/graph/func/mod.rs`:** `Func`'s doc says the `FuncRef` is what a compiled node
  runs under, where the tip still says a node is authored against it (commit 5).
- **`scenarium/src/exec/compile/tests/mod.rs`:** the wire-interning test's doc names the output's
  position by its id, not "the port index the authored wire named" (commit 5).
- **`scenarium/src/exec/compile/tests/mod.rs`:** `a_port_lands_at_its_declared_position_whatever_its_number`,
  a func numbering its ports against their order (commit 5).
- **`scenarium/src/graph/tests.rs`:** `a_load_attaches_each_stub_that_resolves_again_with_the_wiring_that_fits`
  (commit 3), on `GraphSet::reconcile` from commit 6.
- **`darkroom/src/core/document/tests.rs`:** `a_subgraph_path_is_checked_and_mapped` (commit 6).
- **`darkroom/src/core/open_document/tests.rs`:** the unknown-type test also checks the text of
  the repair it reports inside the subgraph (commit 6).
- **`darkroom/src/core/clip/{mod,error,tests}.rs`, `gui/frame/clip_commands.rs`:** a paste lands
  on a copy of the document first and a clip that does not fit pastes nothing, its `PasteError` on
  the status bar, where the tip queues it unchecked and a clip naming a func the library lacks
  panics as a malformed intent; `GraphIntent` and `NewSubgraph` derive `Clone` for it (commit 10).
- **`darkroom/src/core/edit/graph_intent/tests.rs`:** `an_add_past_a_placement_bound_is_refused`,
  which names the depth the add would reach (commits 6 and 9).
- **`darkroom/src/core/edit/graph_intent/mod.rs`:** the module doc says an intent folds into a
  `GraphStep`, not an `UndoStep` (commit 7).
- **`darkroom/src/core/worker_bridge/tests.rs`:** the scenarium imports in one `use` (commit 6).
