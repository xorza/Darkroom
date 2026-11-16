# Subgraphs: the merge as reviewable commits

`subgraphs` is one commit, `0b9370799 Add subgraphs`, over `4eb4ad93d`: 334 files,
+21 976 / −6 562. This plan replays it onto `master` as 12 commits: renames and moves first, then
the shape changes that keep behavior, then the features. Temporary: deleted once the series lands.

## Progress

| # | Commit | State |
|---|---|---|
| 1 | Rename the port declarations `InputDecl` and `OutputDecl` | `78adc05e6`, pushed |
| 2 | Name program nodes by `ProgramId` | `a718d77fe`, pushed |
| 3 | Keep stubs in scenarium's `Graph` | in the worktree |
| 4–12 | | to do |

## Rules for the series

- **Each commit builds and passes the gate** (`cargo fmt --all && … && cargo gate-test`), so the
  series bisects. The cost: commits 3 and 8 hold a few lines of interim code the tip does not
  have, named under each commit.
- **The last commit's tree is the tip's tree**, apart from the lines under Differences from the
  tip. `git diff 0b9370799 <last>` shows only those, which proves nothing else was lost or added
  in the split.
- **A review fix to a line the tip also holds is carried forward**: each later commit keeps it
  rather than taking the tip's line back, and it is listed under Differences from the tip.
- **A file belongs to the commit that writes its final form.** A file that spans several is staged
  by hunk; the lists below mark it *partial*.
- **The two fixture documents are rewritten in every commit that changes the file shape** (2, 3,
  4, 5, 8): `darkroom/sample/sample.darkroom` and `darkroom/src/gui/app/session/bench.darkroom`, each
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
authored node, beside `InstancePath`; every path is a bare node id until commit 7. A disk blob is
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
  turns into its `ProgramId`; commits 8 and 9 widen them to every instance of a node.
- Held back to commit 8: `CompiledGraph::persists_to_disk`, whose one caller is the runtime host
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
- Held back to commit 6: unknown types (`Library::knows`, the load report's `UnknownType`); only
  an interface port can name a type the library lacks.
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
- About +1 250 / −830, mostly mechanical.

## Phase 3: features

### 6. Subgraphs in scenarium: the model

`Subgraph`, an immutable `Interface`, the `Inputs`/`Outputs` boundary nodes, `NodeKind::Instance`,
`NodeDecl` and `Scope`, so every node resolves to a declaration and `Graph::admit` stays the one
admission rule. `GraphSet` holds the main graph and the subgraphs with every rule across them:
scope, uses, recursion, the depth and size bounds (`admit_nodes`), validation, the load's
reconcile (detach, attach, loops, bounds, missing boundary nodes), unknown types, the digest.
`GraphId`, `NodeAddr` and `SubgraphId` join the identity module.

- Owns: `scenarium/src/graph/{graph_set/,interface,node_decl,scope,subgraph}.rs`,
  `graph/node/mod.rs`, `graph/error.rs`, `graph/reconciled.rs`, `graph/mod.rs`, `graph/tests.rs`,
  `internals/test_graph/{mod,sets}.rs`, `internals/mod.rs`, `lib.rs`.
- Held back to commit 10: `interface_change.rs` with `plan_interface`/`write_interface`, whose
  only caller is the editor's interface edit.
- About +3 700 / −500, of which `graph_set/tests.rs` is 1 000. The largest; a split between model
  and reconcile was refused, as neither half has a caller or a test without the other.

### 7. Compile flattens subgraphs

`Compiler::compile(&GraphSet, &Library)` expands each instance by `InstancePath`; a binding that
reaches a boundary or an instance is lowered to a func's output, a constant or nothing; a
const-only `Inputs` port lowers to its constant; a disabled instance disables every node inside.
Plan and execute see no hierarchy.

- Owns: `scenarium/src/exec/compile/mod.rs`, `compile/tests/{mod,flatten}.rs`.
- About +730 / −140.

### 8. The editor holds every graph of a document

`Document { graphs: GraphSet, main_view, sides }`, each `SubgraphSide` holding its view alone
here; `GraphSlot` borrows one graph and its view apart. Intents and undo steps name their graph
(`DocumentRequest::Graph { graph, intent }`, `UndoStep::Graph { graph, step }`), and
`EditFacts.touched` says which graphs an edit wrote. The load repairs every view; the runtime host
compiles the set.

- Owns: `darkroom/src/core/document/{mod,error,graph_revision,layout_file,internals,tests}.rs`,
  `core/edit/{document_queue,document_request,edit_facts}.rs`, `core/edit/step/` apart from the
  subgraph steps, `core/edit/undo_history/tests.rs`, `core/edit/graph_intent/`,
  `core/open_document/`, `core/runtime_host/`, `internals/fixture.rs`, both fixture documents.
- Partial: `core/document/load_report.rs`, `subgraph_side.rs` (the view alone).
- **Interim code:** each GUI call site passes `GraphId::Main` until commit 9 gives it its pane's
  graph.
- About +2 750 / −1 200.

### 9. The window shows a pane per graph

`DocCtx` is what the surround reads and `GraphCtx` narrows it to one graph. `MainWindow` keeps a
`GraphUi` per open graph tab (`TabRef::Graph(GraphId)`). The window has one active graph, set by a
picked tab or a pane's input; menus, status bar and inspector act on it. `Enter` navigates into a
subgraph through an instance, by double click or a name in the path. `ViewCommand::Frame` names
its graph. The driver's targets and the MCP server reach any graph's pane through its tab.

- Owns: `darkroom/src/gui/frame/{doc_ctx,requests/,app_command}.rs`, `gui/graph_ctx/`,
  `gui/main_window/{mod,dock_panes,status_bar/mod}.rs`, `gui/app/session/{mod,internals,
  tests}.rs`, `gui/pane/graph_ui/{mod,internals,canvas,canvas_bg,toolbar,wid}.rs`,
  `gui/pane/graph_ui/gesture/` apart from the files of commits 5, 10 and 11,
  `gui/pane/graph_ui/node_ui/{header/,preview_row}.rs`, `gui/pane/inspector.rs`,
  `core/document/{tab_ref,inspect_target}.rs`, `driver/`, `app/src/mcp/`.
- About +2 900 / −1 600, mostly the context split.

### 10. Edit subgraphs

`SubgraphIntent`: create, rename, set interface, Make Unique, Remove Unused, with steps
`SetSubgraph`, `SetInterface` and `SetDefinition` in the same undo entry as their batch's graph
edits. `InterfaceChange` is scenarium's reversible record and drops wires in every graph. A wire
dropped on a boundary node becomes a port (`port_edit.rs`); the inspector's interface card orders,
renames, retypes and removes ports. The pane header shows the instance path, the use count and
Make Unique; the palette lists the document's subgraphs. An instance rolls up the worst status
and summed time inside, and its footer names the node that failed. A refused edit names why on
the status bar (`EditRefusal`). An undo brings its graph to the front.

- Owns: `scenarium/src/graph/interface_change.rs`,
  `darkroom/src/core/edit/{subgraph_intent/,port_edit,make_unique,error}.rs`,
  `core/edit/step/{set_subgraph,set_interface,set_definition,undo_step,tests}.rs`,
  `gui/pane/interface_card.rs`, `gui/pane/graph_ui/pane_header.rs` (without Publish),
  `gesture/new_node_ui/` (without the shared rows), `gesture/node_menu_ui/`,
  `node_ui/node_footer.rs`, `gui/state/run_state/`, `gui/main_window/menu_bar.rs` (without Export),
  `status_bar/tests.rs`.
- About +3 200 / −300.

### 11. Move node sets: Duplicate, Copy, Paste, Collapse, Expand

One `Fragment` captures nodes with the wires among them, their places and ties, remaps ids and
lands them in a graph; Duplicate is a capture and a paste. A `Clip` is a fragment plus the
subgraphs it uses, RON under a header line on the system clipboard. Collapse makes a subgraph and
one instance of it, refused when a wire leaves the selection and another returns; Expand
reverses it. Bringing a definition in (`bring_in.rs`) reuses an equal one by digest, takes a
different one in beside it with a fresh id, and strips its output paths.

- Owns: `darkroom/src/core/edit/{fragment,bring_in}.rs`, `core/edit/collapse/`, `core/clip/`,
  `core/mod.rs`, `gui/frame/{clip_commands,shortcuts}.rs`, `gesture/shortcuts.rs`.
- About +1 630.

### 12. Share subgraphs: the library folder, Publish and Update, and Export with resources

A library folder, set in Preferences, watched through `notify` and scanned into a
`SharedLibrary`; the palette lists its shared subgraphs. A side's `shared` and `synced` read sync
state: in sync, update available, changed here, changed in both. Update from Library is an
undoable edit; Publish to Library arms, then confirms, in the pane header. Export writes a
document, or a `.darkroomz` bundling the resources the export dialog picks under
`resources/<digest>/<name>`, which unpack checked and read-only beside the archive.
`darkroom/AGENTS.md` gains the archive's line; `darkroom` takes `notify` and `blake3`.

- Owns: `darkroom/src/core/shared_library/`, `core/io/{export,mod}.rs`, `core/io/document/`,
  `core/io/preferences/`, `core/edit/step/set_side.rs`, `core/status_log/mod.rs`,
  `gui/app/{mod,export_dialog}.rs`, `gui/dialogs/mod.rs`, `gui/frame/app_ctx.rs`,
  `gui/pane/preferences.rs`, the rest of `pane_header.rs`, `menu_bar.rs` and `palette_rows/`,
  `darkroom/Cargo.toml`, `Cargo.lock`, `darkroom/AGENTS.md`, `.notes/{subgraphs,TODO}.md`.
- About +1 500 / −120.
- Two features joined to stay at 12; they split at `core/io/export.rs` and
  `io/document/resources/` if a 13th is wanted.

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
- **`scenarium/src/graph/mod.rs`:** the module doc lists `ports` among the modules beside the
  graph (commit 1); commit 6 keeps it when it rewrites that paragraph.
- **`darkroom/src/gui/state/run_state/tests.rs`:** the helper that makes a test's program id is
  `program_id(n)`, not `node_id(n)` (commit 2); commit 10 keeps the name.
- **`darkroom/src/core/document/tests.rs`:** a test that the load closes a viewer whose path
  goes through an instance (commit 2).
- **`scenarium/src/graph/tests.rs`:** `a_load_attaches_each_stub_that_resolves_again_with_the_wiring_that_fits`
  on one graph (commit 3); commit 6 moves it onto `GraphSet::reconcile` beside the tip's
  set-wide test, or drops it where that test covers each case.
