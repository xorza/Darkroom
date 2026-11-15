# Subgraphs

A proposal: a node that is a graph, edited in its own pane, reused inside a document and shared
as a file. It is temporary. When the work lands, each decision moves to the `AGENTS.md` or the
module doc of the system it governs, and this file goes.

## Names

- **A subgraph is a graph with an interface, held by a document.** A node of it is an
  *instance*. Refused: "group", which stays free for the visual node groups of `TODO.md`;
  "function", as a `Func` is a library declaration with a lambda behind it.
- **`Inputs` and `Outputs` are the boundary nodes** of a subgraph, and its *interface* is the
  ports they show.
- **The library folder** holds what is shared between documents; a document's own subgraphs are
  *the document's*. `Library` stays scenarium's registry of funcs and types.
- **A program node** is a node of the compiled program; an authored node inside a subgraph makes
  one per instance.

## Decisions

### The model

- **One kind of subgraph.** Every subgraph is a definition the document holds, and every node of
  it is an instance; a subgraph used once behaves as a private one. As Blender's node groups,
  shared data-blocks with a user count
  ([node groups](https://docs.blender.org/manual/en/latest/interface/controls/nodes/groups.html),
  [data-blocks](https://docs.blender.org/manual/en/latest/files/data_blocks.html)),
  [Grasshopper's clusters](https://www.grasshopper3d.com/page/clusters) and
  [Node-RED's subflows](https://nodered.org/docs/user-guide/editor/workspace/subflows).
  Refused: a private group beside a shared function, as Unreal's collapsed graph beside its
  function
  ([collapsing graphs](https://dev.epicgames.com/documentation/en-us/unreal-engine/collapsing-graphs-in-unreal-engine))
  and Nuke's Group beside its gizmo
  ([gizmos](https://learn.foundry.com/nuke/content/comp_environment/configuring_nuke/creating_sourcing_gizmos.html)):
  two routes to one result, a graph stored by value in one and by id in the other.
- **The hierarchy lives in scenarium.** The authoring model holds the definitions and the
  compile flattens them, as MaterialX keeps node graphs in its core and flattens there
  ([`NodeGraph::flattenSubgraphs`](https://www.sidefx.com/docs/hdk/class_node_graph.html)).
  Refused: a hierarchy only darkroom knows, which has to write a `Func` for each definition into
  a `Library` the load contract takes as fixed, and keep the one table that maps a report back.
- **Every node still resolves to a `Func`.** A subgraph node takes its ports from the
  definition's interface, and the boundary nodes take the same ports mirrored; none has a lambda,
  as the compile removes all three. So `Graph::admit` stays the one admission rule.
- **A node id is unique inside its graph, and an authored node is addressed by its graph and its
  id.** The graph's map of nodes makes that so by construction, and a copy of a definition keeps
  its node ids, so it stays equal to its source. As ComfyUI, where a node id is "local to its
  immediate graph level" and a node is located as `<uuid>:<localId>`
  ([subgraphs](https://docs.comfy.org/custom-nodes/js/subgraphs)), and Blender, where a plain
  tree copy keeps its node identifiers
  ([persistent node identifiers](https://projects.blender.org/archive/blender-archive/commit/90ea1b76434fe175)).
  Refused: ids unique across the document, a rule a file can break with no sound repair, and one
  that makes every copy mint ids and so differ from its source for good.
- **One `Inputs` node and one `Outputs` node.** Each port is a row; a blank last row takes a
  dragged wire and becomes a port; the inspector orders, renames and removes them; neither node
  can be deleted. As Blender ("drag a link from or to the hollow socket"), Unreal's function
  entry and result, and [ComfyUI](https://docs.comfy.org/interface/features/subgraph). Refused: a
  node per port, as Nuke's
  [Input](https://learn.foundry.com/nuke/content/reference_guide/other_nodes/input.html), whose
  order needs a second rule, a hidden number that scripts repair
  ([pipe numbers](https://www.nukepedia.com/knowledge/general-tutorials/written-tutorials/working-with-pipe-numbers/)).
- **A port's type is inferred when the port is made, then held as its declaration.** The first
  wire names the type, the name and the default; a collapse reads them off each wire that
  crosses the selection; the inspector changes one on request. As Blender's hollow socket.
  Refused: a type derived from the inner wiring on every edit, where removing the last inner
  wire breaks wires on instances in other graphs, against "An output's type is its declaration";
  a type picked by hand first, as Unreal's Function Input
  ([material function expressions](https://dev.epicgames.com/documentation/unreal-engine/material-function-expressions-in-unreal-engine)),
  two picks where a drag is one.
- **A wire from `Inputs` to `Outputs` is an ordinary wire.** The port it leaves has a type
  already. The one thing refused is a blank row wired to a blank row, which names no type.
- **An interface port is an id and a declaration.** The declaration is the `FuncInput` or
  `FuncOutput` of the port that made it, so a const-only input, a picker and an enum are exposed
  as they are; the id lets an interface edit or a library update move each instance's bindings to
  the right index. Refused: matching by name, which darkroom's stub decision refuses already;
  the index alone, which a reorder breaks.
- **An interface edit rewrites every instance in the same undo step.** A removed port takes the
  wires on it in every graph of the document, a retyped one the wires it no longer admits, and
  undo puts them back. A node keeps the signature of its definition, as it keeps a func's, so a
  mismatch can only come from a file, and the load makes that node a stub.
- **A port made from an `Any` input is `Any`**, and feeds only `Any` inputs, by the existing
  wire rule. Where several inputs make one port, it takes the narrowest of their types.
- **Subgraphs do not recurse.** A definition holds no node of itself, directly or through
  others; the edit gate refuses the add, the paste and the update that would close the loop. As
  Blender ("Recursive node groups are prohibited") and Substance Designer, where cyclical
  dependencies are forbidden
  ([graph instances](https://helpx.adobe.com/substance-3d-designer/substance-model-graphs/graph-instances-in-substance-model-graphs.html)).
- **Nesting stops at eight levels and the flattened size has a bound, both held at the edit
  gate.** Two instances a level over thirty levels are 2³⁰ program nodes, and a compile has no
  expected failure, so the edit that would pass a bound is refused. Eight, as a path of eight
  ids is 128 bytes and stays a plain value. Cost: a ninth level is refused.
- **A wire through an instance ties every output to every input** in the cycle check. Cost: a
  loop through two ports that share nothing inside is refused, as Blender refuses it.
- **Subscriptions stay inside one graph.** An interface has no event ports yet.

### Compile and run

- **The compile flattens; plan and execute see no hierarchy.** A subgraph node and a boundary
  node emit nothing, and a binding that reaches one is replaced by what the other side holds,
  until it names a func's output, a constant or nothing. So a passthrough costs nothing, output
  demand prunes across the boundary and each inner node caches as any node does. As Unreal
  expands each macro instance "into a unique set of nodes during compilation".
- **A program node's id is its path**: the instances from the main graph down, then the node.
  It is unique by construction, the same on every compile, and reads back as the place to show;
  in the program's sorted order one instance's nodes are one run, so its status needs no table.
  As ComfyUI's execution id, "a list of all subgraph node IDs, followed by the actual original
  node ID" ([subgraphs](https://docs.comfy.org/custom-nodes/js/subgraphs)). Refused: a hash of
  the path, as Blender's compute context hash
  ([`BLI_compute_context.hh`](https://projects.blender.org/archive/blender-archive/src/commit/5cc793912efe4432b51cc7ddda4005c56fcbc93b/source/blender/blenlib/BLI_compute_context.hh)),
  which the program would hold beside the path it still needs to name a failed node; a root
  node's own id beside hashed inner ones, which a file can make collide.
- **A program id is its own type, a path of bounded length held inline.** Reports, seeds,
  evictions and viewer tabs carry it with no allocation; `NodeId` names an authored node in its
  graph.
- **A disk blob is named by the digest of its path and holds the path**, which a read checks, as
  a file name has a length limit and a path of eight ids passes it.
- **A disabled instance disables every node inside it.** An instance has no cache mode: the
  nodes inside carry their own.

### The editor

- **A subgraph edit is a document edit.** One undo history, and the change is in every instance
  at once; inside a document there is no publish step. As Blender, and as Houdini where "other
  instances of the same asset will get the same changes"
  ([editing assets](https://www.sidefx.com/docs/houdini/assets/edit.html)). Refused: a draft
  with Apply, a mode to enter and leave.
- **An undo brings the graph it changes to the front** when its tab is open, and names it in the
  status bar when not, so no undo changes only what is off screen.
- **Duplicate and paste make another instance; Make Unique gives the selected instances a
  definition of their own.** As Blender, Grasshopper ("the two instances of the cluster will be
  entangled") and ComfyUI, whose subgraphs are "linked rather than duplicated" by default
  ([release notes](https://blog.comfy.org/p/subgraph-official-release)). Refused: a copy by
  default, as Nuke's Group, where ten copies of one pipeline are ten graphs to fix.
- **Make Unique is shallow.** The new definition has a fresh id and the same nodes under the
  same ids, and its inner instances name the definitions they named. As Blender's single-user
  copy, which duplicates one data-block. A deep copy is Make Unique again, one level in.
- **The count of uses always shows**, on the node and in the pane header once it passes one,
  with Make Unique beside it. Grasshopper's users report "no visual sign of entanglement"
  ([forum](https://discourse.mcneel.com/t/cluster-disentangled-without-notice/57955)); Blender
  shows the number.
- **One pane per graph.** The main graph has its tab and each subgraph at most one; opening it
  again focuses that tab. A definition has one view: its placements, ties, camera and selection.
- **A pane shows its subgraph through one instance.** Entered from a node, it shows that
  instance's statuses, previews and timings, and its header is the path of names from the main
  graph; opened from the library list it shows none. As Blender's Tab and ComfyUI's navigation
  bar.
- **Collapse and Expand are inverses.** Collapse turns the selection into a new subgraph and one
  node of it, a port for each source or consumer a crossing wire names; Expand puts one
  instance's nodes back with fresh ids. As Blender's Make Group and Ungroup, ComfyUI's convert
  and unpack.
- **A collapse is refused when a wire leaves the selection and a later one returns**, and the
  status bar names the node between: the instance would feed itself. A boundary node and a stub
  in the selection stay where they are.
- **An unused subgraph stays until removed.** The library list marks it, and Remove Unused clears
  them. Refused: removal with the last instance, which loses work in Blender unless a fake user
  is set. Cost: a collapse and an expand leave one behind.
- **An instance shows the worst status inside it and the time summed**, and its footer names the
  node that failed.
- **Copy and paste go through the system clipboard as text.** A clip is a document fragment in
  RON under a header line: the nodes, the wires among them, their ties and places, and each
  subgraph they use. A paste mints node ids for the graph it lands in and takes the subgraphs in
  by the rule under Sharing; text with no header, or past the size bound, is not a clip and
  pastes nothing. Cost: a clip holds a const path as the document does.

### Sharing

- **A document keeps a copy of each library subgraph it uses**, and of those that one uses,
  under the same ids, so it opens anywhere. As Blender's Append ("the asset and all its
  dependencies will be Appended (copied) into the current file",
  [asset browser](https://docs.blender.org/manual/en/latest/editors/asset_browser.html)) and
  Houdini's definitions saved in the scene
  ([save asset](https://www.sidefx.com/docs/houdini/ref/windows/save_asset.html)). Refused: a
  link, as Nuke's gizmo, where a script stores "only the gizmo's name and control settings": a
  shared document then needs the library beside it, and a result changes with a file the
  document does not hold.
- **A library file is an ordinary document.** The library is a folder of `.darkroom` and
  `.darkroomz` files, and each subgraph a file marks as shared shows in the palette; the file's
  main graph is where the author wires the subgraph to sample inputs and previews. Export writes
  such a document. As Blender's asset library, a folder of blend-files whose data is marked as
  an asset
  ([asset libraries](https://docs.blender.org/manual/en/latest/files/asset_libraries/introduction.html)).
  Refused: a format of its own, as Nuke's `.gizmo` and Grasshopper's `.ghcluster`, with no place
  for the wiring that shows a subgraph working.
- **The folder is read off the frame, once at start and when it changes.** A file that does not
  load is listed with its error; an id two files hold is one entry when their digests are equal
  and one per file, each under its file's name, when not.
- **Identity is the id, difference a digest.** The digest covers the interface, the graph, the
  name and description, and each subgraph it uses by that one's digest, as a git tree names its
  children ([git objects](https://git-scm.com/book/en/v2/Git-Internals-Git-Objects)); not its
  own id, and not its view, so moving a node changes nothing.
- **A copy remembers the digest it last shared with the library.** Against the two current
  digests it reads in sync, update available, changed here, or changed in both, the last of
  which a two-way comparison cannot tell from the others. Refused: version numbers, which order
  two copies and say nothing of a third made beside them.
- **Nothing but an explicit Update changes a definition the document holds.** Taking in an equal
  one under an id the document has reuses it; a different one under that id comes in beside it
  with a fresh id, and what came with it names that one. As Cargo builds "two separate copies"
  of a dependency two packages need at incompatible versions
  ([resolver](https://doc.rust-lang.org/cargo/reference/resolver.html)). Refused: one version
  for the document, which changes a subgraph nobody touched.
- **Update from Library is an undoable edit; Publish to Library asks first.** An Update moves
  each instance's bindings by port id and reports each wire it drops. A Publish replaces the
  definition inside the library file and leaves that file's main graph; it names what it
  overwrites when the library changed since the last sync, and writes the file whole or not at
  all. As Simulink's Restore Link and Push Link
  ([disable or break links](https://www.mathworks.com/help/simulink/ug/disable-links-to-library-blocks.html)),
  Houdini's Match Current Definition and Save Node Type.
- **A shared file holds no code.** It is graphs over the built-in funcs; a func the receiver
  lacks loads as a stub inside the subgraph, as in any document.
- **A definition that enters from another file loses its output paths**, whichever way it comes:
  a write to a path inside a subgraph is one the receiver did not pick and cannot see. A node
  pasted onto the canvas keeps its own, which is in view.

### Resources and the archive

- **An export lists every path the subgraph holds, and each is bundled or stripped.** The dialog
  shows each const path of the definition and of those it uses, with its size. Stripped is the
  default: an absolute path names the author's system and nothing on the receiver's.
- **An export with resources is one `.darkroomz`**, the archive holding `document.ron` and each
  bundled file under `resources/`; an export with none is a document of either form. As Blender
  packs external files into the blend-file
  ([packed data](https://docs.blender.org/manual/en/latest/files/blend/packed_data.html)).
  Refused: a folder of the document and its files, as Unreal's Migrate writes
  ([migrating assets](https://dev.epicgames.com/documentation/en-us/unreal-engine/migrating-assets-in-unreal-engine)),
  which is two things to send.
- **Resources unpack beside the archive**, into `name.darkroomz-resources/`, as the disk cache
  sits in `name.darkroomz-cache/`. A path in memory is then an absolute path as any other, and
  no node, codec or digest changes. As Unity's Import Package unpacks into the project
  ([importing packages](https://docs.unity3d.com/6000.6/Documentation/Manual/AssetPackagesImport.html)).
  Refused: reading the archive in place, as Houdini's
  [`opdef:`](https://www.sidefx.com/docs/houdini/assets/opdef.html) paths do, which every path
  reader then has to know, LibRaw and ONNX Runtime included; a temp folder per session, which
  unpacks at every open and gives a path no disk cache entry outlives. Cost: the bytes are on
  disk twice.
- **An entry is named by the digest of its content**, `resources/<digest>/<file name>`, a folder
  by the digest of its files. Two files of one name do not clash, one file bundled twice is one
  entry, and a changed file is a new entry. Refused: the file name alone with a size and
  modified-time check, which is a guess about what a name holds.
- **An unpacked file is whole, checked and read-only.** It is written under a temporary name,
  held to the digest in its entry's name and renamed into place, so a file that is there is the
  one the archive holds and an interrupted unpack leaves nothing to trust. Read-only as Go keeps
  its module cache ([module cache](https://go.dev/ref/mod#module-cache)); Cargo marks a finished
  unpack with `.cargo-ok` for the same question
  ([`RegistrySource`](https://doc.rust-lang.org/nightly/nightly-rustc/cargo/sources/registry/struct.RegistrySource.html)).
- **An entry unpacks when a path that names it enters memory**: every one at the open of the
  archive, and those a subgraph names when it is copied from the library. A folder under
  `-resources` that no entry names is removed then.
- **A bundled path reads `resources/…` in the file and names an entry**; a relative path that
  names none lies beside the file, as now. A save of a `.darkroomz` writes an entry for each
  const path inside its resources folder, so an archive opened and saved keeps what it had.
- **Only a regular file under `resources/` is unpacked.** An entry with `..` in its name, an
  absolute one, a link, or one longer than its header declares is refused and named in the load
  report: an archive is another person's data. The name goes through the zip crate's
  `enclosed_name`, and no link is written, as its own extraction once wrote through one
  ([CVE-2025-29787](https://www.sentinelone.com/vulnerability-database/cve-2025-29787/)).
- **A resources folder that cannot be written is an error the load report names**, and the paths
  into it stay unresolved. Refused: a second place to unpack to, which gives one archive two
  behaviors.
- **An output path is always stripped**, as it names no file to bundle.
- **A stripped input takes the receiver's library default when the subgraph enters a document**,
  as a new node's input does, so a model path follows the receiver's preferences.
- **A path the receiver must choose is an interface input.** Its value then sits on the
  instance, outside the definition, and an export has nothing to strip.
- **A document that copies a subgraph from an archive names the unpacked file where it lies**,
  in the library folder. Cost: sending that document on means exporting it with the resource
  again.

## Not in the first version

- Event ports on an interface.
- A cache mode on an instance.
- One computed value shared by instances whose digests are equal.
- Several `Inputs` nodes in one subgraph.
- A linked library, and a locked or password-protected subgraph.
- Packing resources into a document from a command of its own, apart from an export.

## Order of work

1. scenarium: definitions, the subgraph node and the boundary nodes, the graph in an authored
   address, admission, the recursion, depth and size gates, the flatten and the program path. A
   test holds the program of a graph with a subgraph to that of its hand-inlined twin.
2. darkroom's core: the document holds subgraphs and their views, an intent names its graph, the
   interface steps, the load per graph.
3. Panes: the tab, the header path, the instance a pane shows through, run state by program id.
4. Collapse, Expand, the boundary nodes on the canvas, the interface in the inspector.
5. Duplicate, Make Unique, the clipboard.
6. The library folder, the palette, the digests, Update and Publish.
7. Export: the path list, the archive's resource entries, the unpack.
