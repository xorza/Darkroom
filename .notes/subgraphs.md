# Subgraphs

A proposal: a node that is a graph, edited in its own pane, reused inside a document and shared
as a file. It is temporary: each decision moves to the `AGENTS.md` or the module doc of the
system it governs, and this file goes.

## Names

- **A subgraph is a graph with an interface, held by a document.** A node of it is an
  *instance*. Refused: "group", which stays free for the visual node groups of `TODO.md`;
  "function", as a `Func` is a library declaration with a lambda behind it.
- **`Inputs` and `Outputs` are the boundary nodes** of a subgraph, and its *interface* is the
  ports they show.
- **A declaration is what a node resolves to**: a func, a subgraph for its instance, or a
  subgraph for one of its boundary nodes. Its *ports* are the inputs and outputs it shows and is
  wired by, and a *port id* names one of them among the ports of its side.
- **A file kind** is a registered set of file extensions under a name, as "Image" or "Raw".
- **The graph set** is every graph of a document, its main graph and its subgraphs.
- **The library folder** holds what is shared between documents; a document's own subgraphs are
  *the document's*. `Library` stays scenarium's registry of funcs and types.
- **A program node** is a node of the compiled program; an authored node inside a subgraph makes
  one per instance. An *instance path* is the instances from the main graph down to a graph.
- **A stub** is a node its graph cannot hold, kept in that graph as the record a detach leaves.
- **A side** is what the editor keeps beside one subgraph: its view, the instance its pane shows
  it through, and its place in the library.
- **The active graph** is the one the window's commands act on.
- **A fragment** is a set of nodes as it moves: what Duplicate, Copy, Paste, Collapse and Expand
  carry.

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
- **One type owns every graph of a document.** `GraphSet` holds the main graph and the
  subgraphs, and each rule that lies across graphs is its method: the scope of a graph, the
  recursion and the bounds, an interface edit, the load's repair. Refused: the main graph and
  the subgraphs as two parameters, which no call can pass apart.
- **Every node resolves to a declaration; a func is a declaration with a body.** `Ports` is
  what admission and the editor read, so `Graph::admit` stays the one admission rule. A `Func`
  holds its ports beside a lambda that is never optional, and the compile reads a body off a
  func alone. As MaterialX, where a `nodedef` declares the ports and an implementation, code or
  a nodegraph, stands behind it, and "the only requirement is that the nodegraph and nodedef
  have matching inputs and outputs"
  ([node definitions](https://kwokcb.github.io/MaterialX_Learn/documents/node_definitions.html)).
  Refused: a `Func` with an optional lambda for an instance and a boundary node, which gives a
  declaration an id, a behavior and a cache mode it has no value for, and gives every reader of
  a body a case that cannot happen.
- **A node id is unique inside its graph, and an authored node is addressed by its graph and its
  id.** The graph's map of nodes makes that so by construction, and a copy of a definition keeps
  its node ids, so it stays equal to its source. As ComfyUI, where a node id is "local to its
  immediate graph level" and a node is located as `<uuid>:<localId>`
  ([subgraphs](https://docs.comfy.org/custom-nodes/js/subgraphs)), and Blender, where a plain
  tree copy keeps its node identifiers
  ([persistent node identifiers](https://projects.blender.org/archive/blender-archive/commit/90ea1b76434fe175)).
  Refused: ids unique across the document, a rule a file can break with no sound repair, and one
  that makes every copy mint ids and so differ from its source for good.
- **One `Inputs` node and one `Outputs` node, each a node told by its kind.** Each port is a
  row; a wire dropped on the node's body becomes a port and its wire in one entry; the inspector
  orders, renames and removes them; neither node can be deleted. Placement, selection and wiring
  treat them as any node. As Blender ("drag a link from or to the hollow socket"), Unreal's function entry and
  result, and [ComfyUI](https://docs.comfy.org/interface/features/subgraph). Refused: a node per
  port, as Nuke's
  [Input](https://learn.foundry.com/nuke/content/reference_guide/other_nodes/input.html), whose
  order needs a second rule, a hidden number that scripts repair
  ([pipe numbers](https://www.nukepedia.com/knowledge/general-tutorials/written-tutorials/working-with-pipe-numbers/));
  two reserved node ids, as ComfyUI's `-10` and `-20`
  ([node ids](https://docs.comfy.org/custom-nodes/js/node-id-migration)), which make a lookup
  free and rule out several `Inputs` nodes later.
- **A type is a small value that names what it accepts.** `DataType` is `Copy` and holds ids
  alone: a scalar, a directory, or a reference into the library's registry, which is an enum, a
  custom type or a file. A file type is a file kind and a mode; the mode decides the kind of
  value, one path or several, and which dialog opens. As MaterialX, whose types are `typedef`
  elements of the library, a port's `type` being a typedef's name and `filename` one of them
  ([specification](https://github.com/AcademySoftwareFoundation/MaterialX/blob/main/documents/Specification/MaterialX.Specification.md)).
  Refused: a dialog's mode and extension list inside the type behind an `Arc`, which the file
  then has to serialize; the extensions on the input beside its picker variants, as Unreal's
  `FilePathFilter` metadata
  ([property specifiers](https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-engine-uproperties)),
  which leaves the type saying nothing of what it accepts.
- **A file kind is registered in the library, as an enum and a custom type are**: a name and
  its extensions, under an id. The kind is registered, not the whole file type, so "Image" is
  one entry for the port that opens one, the port that opens several and the port that saves.
  A kind with no extensions is every file.
  As Apple's type identifiers, each a registered id with its extensions
  ([uniform type identifiers](https://developer.apple.com/documentation/uniformtypeidentifiers.md)).
- **A file port feeds another when it holds the same kind of value and its file kind is a subset
  of the other's.** So a "PNG" output feeds an "Image" input and not the reverse, and a
  directory feeds a directory. The narrowest of several file types is the one whose kind is a
  subset of all the others; where none is, they do not share a port. Apple and the freedesktop
  database declare this relation by hand, `public.png` conforming to `public.image` and
  `sub-class-of`
  ([shared MIME-info](https://specifications.freedesktop.org/shared-mime-info/0.18/ar01s02.html)),
  as their types are told by content; here a kind is its list, so the relation is computed and
  cannot drift. A constant path is still checked for its kind of value alone: extensions vary in
  case and some files have none.
- **Each kind of registered type has its own id type and its own table**: `EnumId`,
  `CustomTypeId`, `FileKindId`. Refused: one `TypeId` for all, where the variant in the type and
  the entry in the registry can disagree, and `Library::add` and `register_type` each assert
  that they do not; a bare `Named(TypeId)`, where every question about a type needs the library
  and an unknown id does not say what kind of value it holds.
- **A type the library lacks is a type like any other, equal only to itself.** A document names
  a type in one place, the declaration of an interface port, and a newer version, a build
  without a feature or a shared subgraph can name one this build does not register. The port,
  its bindings and its constants stay as written. An unknown custom type wires by its id and has
  no disk cache; a constant of an unknown enum is kept unchecked and shown read-only; an unknown
  file kind filters nothing in the dialog and wires only to itself; an `Any` input accepts all
  three. The load report names each unknown type and the ports that use it, and the next load
  that has the type checks each port as any other. The same promise a stub makes for a node with
  no func: kept, inert, shown with its reason, live again when the library has it. Refused: a
  stub for each instance whose interface names the type, which disables a whole subgraph over
  one port; taking the port out, as nothing has to move for it to stay.
- **A library registers every type its funcs declare**, checked once when it is composed. So a
  type the library lacks is one no func here declares, and no body reads a value of it: it only
  passes through other ports of that type into stubs or an `Any`. This replaces the checks
  `Library::add` and `register_type` each make of the other's entries, as the two may arrive in
  either order.
- **No value held by shared ownership is serialized.** A write copies the value once per holder
  and a read gives each holder its own, so the sharing the type states is gone. Refused: serde's
  `rc` feature, which "does not preserve identity and may result in multiple copies of the same
  data" ([feature flags](https://serde.rs/feature-flags.html#rc)) and which Cargo's feature
  unification
  ([features](https://doc.rust-lang.org/cargo/reference/features.html#feature-unification))
  turns on for every `Rc` and `Arc` in every crate; `#[serde(with)]` on the one field, which
  serializes through the pointer and leaves a heap value inside a type.
- **A port's type is inferred when the port is made, then held as its declaration.** The first
  wire names the type, the name and the default; a collapse reads them off each wire that
  crosses the selection; the inspector changes one on request. As Blender's hollow socket.
  Refused: a type derived from the inner wiring on every edit, where removing the last inner
  wire breaks wires on instances in other graphs, against "An output's type is its declaration";
  a type picked by hand first, as Unreal's Function Input
  ([material function expressions](https://dev.epicgames.com/documentation/unreal-engine/material-function-expressions-in-unreal-engine)),
  two picks where a drag is one.
- **A wire from `Inputs` to `Outputs` is an ordinary wire.** The port it leaves has a type
  already. Refused: a blank last row on each boundary node as the drop target, a placeholder
  among every boundary node's ports, which could be wired to another and name no type.
- **A port's id is part of its declaration.** An `InputDecl` and an `OutputDecl` hold it, unique
  among the ports of their side, and an event holds one too. A func numbers its ports, and a
  number once shipped is never changed or used again, as a protobuf field's
  ([field numbers](https://protobuf.dev/programming-guides/proto3/#assigning)). An interface
  mints a random id for each port, so two copies of a definition changed apart never give one id
  to two ports. An interface port's declaration is that of the port that made it under its own
  id, so a const-only input, a picker and an enum are exposed as they are.
- **An interface holds its ports to the rules a func's ports pass**, apart from the one that an
  output is never `Any`: an id is unique in its list, and a default fits its input, its type or
  its picker. An interface is immutable and built by a checked constructor alone, so the ports its
  two boundary nodes show, which it derives, are never stale. What needs a library, an enum's
  variant, is the load's.
- **A const-only input takes a wire from a const-only `Inputs` port, and from nothing else.** An
  instance holds only a constant there, so the flatten lowers the wire to that constant.
- **An interface edit is scenarium's, and it changes another graph only by the wires it
  drops.** A reorder and a rename change no binding anywhere. A removed port takes the wires on
  it in every graph of the document, a retyped one the wires it no longer admits, and undo puts
  them back: the record is the two interfaces and those bindings. Refused: the interface as a
  field a host sets, which leaves the dropped wires in other graphs to each host.
- **A port made from an `Any` input is `Any`**, and feeds only `Any` inputs, by the existing
  wire rule. Where several inputs make one port, it takes the narrowest of their types.
- **Subgraphs do not recurse.** A definition holds no node of itself, directly or through
  others; the edit gate refuses the add, the paste and the update that would close the loop. As
  Blender ("Recursive node groups are prohibited") and Substance Designer, where cyclical
  dependencies are forbidden
  ([graph instances](https://helpx.adobe.com/substance-3d-designer/substance-model-graphs/graph-instances-in-substance-model-graphs.html)).
- **Nesting stops at eight levels and the flattened size has a bound, both held at the edit
  gate for every graph an add grows.** Two instances a level over thirty levels are 2³⁰ program
  nodes, and a compile has no expected failure, so the edit that would pass a bound is refused.
  An add inside a subgraph grows each graph that places it, so the gate asks them all. Eight, as
  a path of eight ids is 128 bytes and stays a plain value. Cost: a ninth level is refused.
- **A repair reads, then writes.** Each step of the load asks a graph what to change under a
  scope, drops the scope, and changes it. Refused: taking a graph out of the document for the
  call, which leaves the document wrong while a scope reads it.
- **A graph keeps the nodes it cannot hold.** Its stubs sit in the `Graph` beside its nodes, the
  ids of the two disjoint, so a definition is one value wherever it goes: a clip, a library
  file, a copy, a digest. The load is one call, which attaches each stub that resolves again and
  detaches each node that does not. As Blender, which swaps an unknown node for an "undefined"
  one in the tree and keeps its links
  ([undefined node](https://projects.blender.org/blender/blender/pulls/114783)). This replaces
  "Stubs are darkroom's, not scenarium's": what that decision refused, a stub among the nodes
  that every reader has to step around, is still refused, as no reader of `nodes` sees one.
  Refused: the records in the host beside each graph, where a definition travels in two halves,
  the host writes the attach rules a second time, and every edit of a node asks two containers
  which one holds it.
- **Why a node was detached is kept with its stub, and is its own answer, apart from why a node
  does not resolve.** A node detached for a rule across graphs, a loop or a bound, resolves, so
  the reason cannot be read off it again. Every stub whose node resolves is attached at the next
  load, and the load decides again.
- **A binding names a port by its id.** An input, an output and an event are each a node and a
  port id, on a func node, an instance and a boundary node alike, and the same id names a port
  on an instance and on the boundary node inside. A func that gains a port keeps every saved
  node; a port that leaves takes only its own binding, which the load drops and reports as any
  binding the library refuses; a rename and a reorder change nothing. The signature a node held
  of its func's ports goes, and with it the stub for "changed ports". The compiled program stays
  positional: the compile maps each id to its position once. As Blender, which finds a group's
  sockets by identifier when it redirects links
  ([group creation](https://projects.blender.org/archive/blender-archive/commit/fa3ca9afdb6f1ea1b709226af3f085133e5ddc2f)),
  and Unreal, whose pin has a persistent id as "pin name could be changed"
  ([`UEdGraphPin`](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Engine/UEdGraphPin)).
  What darkroom refused stays refused: nothing is matched by name, a renamed port keeps its id,
  and a port whose meaning changes takes a new one. Cost: an id on every func port, a new file
  shape, and a port a func gains starts unbound on a saved node, a required one showing as
  missing. Refused: the port's index with a signature of the func's ports, as ComfyUI's
  `origin_slot` and `target_slot`
  ([workflow JSON](https://docs.comfy.org/specs/workflow_json)), which makes a stub of every
  saved node when its func gains an input and rewrites the bindings and routes of every instance
  when an interface is reordered; ids on interface ports alone, two address forms in one table.
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
- **A program id is its own type, a path of bounded length held inline, and an identity.**
  Reports, seeds, evictions and viewer tabs carry it with no allocation; `NodeId` names an
  authored node in its graph. It lives with the other addresses, so the authoring model imports
  nothing of the execution side.
- **An instance is named by its instance path, not by a program id.** An instance emits no
  program node, so the path down to it is its whole address: what a compile expands under and
  what a pane shows through.
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
- **An intent and an undo step name their graph.** `GraphIntent` keeps its vocabulary and
  travels with a `GraphId`; a step writes one graph and its view, borrowed apart from the rest.
- **An authored thing is keyed by its graph and its node id, a run-time thing by its program
  id**, widget ids included. A copy of a definition keeps its node ids, so two open panes can
  hold the same one.
- **The window has one active graph**: the graph of the pane that last took input. The menu
  bar, the status bar, the selection's inspector and each shortcut act on it, and an edit that
  removes its graph hands it to the main graph. As VS Code's active editor, "the one that
  currently has focus or, when none has focus, the one that has changed input most recently"
  ([`window.activeTextEditor`](https://code.visualstudio.com/api/references/vscode-api)).
  Refused: the main graph's selection as "the" selection, which is what the menu bar, the status
  bar and the inspector read today.
- **The window reads the document, a pane reads one graph.** `DocCtx` is what the surround and
  the dock take; `GraphCtx` adds one graph, its view, its instance path and its camera, each
  resolved once when the context is made, so every accessor stays a field read. Refused: one
  context that is both, which fixes "the graph" for the whole window.
- **A graph pane's state is per graph.** Each open graph has its own `GraphUi`: geometry,
  gestures and the revision its caches were swept at. One pointer still makes one drag, as a
  gesture latches on its own canvas.
- **One fragment is what Duplicate, Copy, Paste, Collapse and Expand move.** Each reads a set of
  nodes with the wires among them, their places and their ties, gives them ids, and lands them
  in a graph. Duplicate is a capture and a paste into the same graph, as Unreal's graph editors
  duplicate by copying and pasting the selection. Refused: the copy written out once per
  command, five times.
- **An edit says which graph it touched**: none, one, or several. A pane lays out again for its
  own graph alone, and an interface edit, which changes ports in every graph that places an
  instance, says several.
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
- **The count of uses always shows** in the pane header once it passes one, with Make Unique
  beside it. Refused: on the instance node, whose header has no room for it beside its title and
  marks. Grasshopper's users report "no visual sign of entanglement"
  ([forum](https://discourse.mcneel.com/t/cluster-disentangled-without-notice/57955)); Blender
  shows the number.
- **A picked tab is input on its pane.** After a change of the pane arrangement the graph the
  focused pane shows becomes active, so the menus act on the graph whose tab was just picked and
  a driver's target, which names the active graph, reaches a subgraph's pane through its tab.
  As VS Code's active editor, the one that "currently has focus". Refused: the active graph
  moved only by an edit, which leaves the menus acting on a pane no longer in front.
- **Entering is navigation.** A double click on an instance, a name in the pane header and the
  status bar's link to a failure inside raise one `Enter`, which sets the side's `through`,
  opens or focuses the tab and makes the graph active, recording nothing; one raised against an
  earlier frame's document enters nothing. Refused: an undoable step, as the camera and the tabs
  stay out of the history.
- **A refused edit names why on the status bar.** An edit a person can ask for that the rules
  refuse, an instance inside itself, a bound passed, a boundary node or a used subgraph removed,
  is an `EditRefusal` beside `MalformedIntent`: it writes nothing and the edit facts carry it to
  the shell, which parks its line. Refused: a silent no-op as for a stale intent, which leaves a
  person guessing; a widget that hides the ask, which cannot see a bound the add would pass.
- **An edit of the set of graphs is a request of its own** (`SubgraphIntent`: a definition in or
  out, an interface), committed into the same history entry as the graph edits of its batch, so
  a collapse, its new definition and the instance that places it are one undo. Its steps write
  the set and every view the change reaches. Refused: a graph intent on the main graph that
  writes other graphs, which hides what it touches.
- **Framing names its graph.** `ViewCommand::Frame` carries the graph, so a toolbar chip frames
  its own pane and the menu the active one. Refused: the active graph read when the command
  runs, which framed whichever pane last took an edit.
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
- **An unused subgraph stays until removed.** The library list marks it, and the Graph menu's
  Remove Unused clears them, outermost first, in one entry: the menu holds the document's
  commands, and the palette adds nodes. Refused: removal with the last instance, which loses work
  in Blender unless a fake user is set. Cost: a collapse and an expand leave one behind.
- **An instance shows the worst status inside it and the time summed**, and its footer names the
  node that failed. Each report is folded into the instances above it when it lands, not read
  off the graph each frame.
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
- **Update from Library is an undoable edit; Publish to Library asks first.** An Update keeps
  each instance's bindings, which name ports by id, and reports each wire it drops. A Publish replaces the
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

- **The folder is watched through `notify`**, already the workspace's watcher in `common::fs`, a
  dependency `darkroom` now takes, so a library file written by another window is listed again
  without a restart. Refused: a poll of the folder's times, which reads the disk every frame or
  answers late.
- **Publish asks in the pane header**, naming the file it writes and, when the library changed
  too, that it overwrites those changes: one click arms it, a second writes. Refused: a modal
  dialog for one yes or no beside the link that asked it.

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
  const path inside any archive's resources folder, under the digest its folder names, so an
  archive opened and saved keeps what it had, under its own name or another.
- **An archive is read where it lies, and a bundled file streams into it from disk**, so the
  document's size limit holds the document entry alone and no save or open holds every bundled
  file in memory. An export's picked files are hashed as they are read, once for the name and
  once into the archive; an unpacked file's digest is its folder's name, so a resave reads it
  once. Refused: the archive read whole, which caps every bundle at the document limit.
- **Export is a Graph menu command on the selected instance, else the subgraph the pane shows**,
  beside Make Unique and Remove Unused, and asks in a modal dialog, as Save does. A pick that
  bundles saves a `.darkroomz` alone, as plain text holds no file.
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

## The shape

What the code holds. A sketch for the work, deleted with this file.

### Scenarium

```rust
// data/data_type: a type is a small value of ids
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataType { Any, Float, Int, Bool, String, Dir,
                    File { kind: FileKindId, mode: FileMode },
                    Custom(CustomTypeId), Enum(EnumId) }
pub enum FileMode { Open, OpenMany, Save }

// library: one table per kind of registered type, each under its own id type
pub struct Library { funcs: HashMap<FuncId, Func>,
                     enums: HashMap<EnumId, EnumType>,
                     custom_types: HashMap<CustomTypeId, CustomType>,
                     file_kinds: HashMap<FileKindId, FileKind>,
                     codecs: Arc<Codecs> }
pub struct FileKind { name: String, extensions: Vec<String> }
// check(): every type a func declares is registered; asserted once when a library is composed

// graph/identity.rs: every address
pub struct InputPort  { node_id: NodeId, port: PortId }
pub struct OutputPort { node_id: NodeId, port: PortId }
pub struct EventPort  { node_id: NodeId, event: PortId }
pub struct InstancePath { .. }   // at most 7 ids, empty for the main graph
pub struct ProgramId    { .. }   // an InstancePath and a node: at most 8 ids; in a file, its list of ids

// graph/ports.rs: what any node shows and is wired by; plain data, in the file as in memory
pub struct InputDecl  { id: PortId, name, data_type, required, const_only, default_value,
                        value_variants, description }
pub struct OutputDecl { id: PortId, name, data_type, description }
pub struct Ports      { inputs: Vec<InputDecl>, outputs: Vec<OutputDecl> }   // an id is unique in its list

// graph/func: ports with a body; the library's, never in a file
pub struct Func { id: FuncId, name, category, description, sink, default_cache_mode, behavior,
                  ports: Ports, events: Vec<FuncEvent>, lambda: FuncLambda }
// builder: .input(1, InputDecl::required(..)), .output(1, OutputDecl::new(..)), .event(1, ..)

// graph/interface.rs: immutable, built by `checked` alone
pub struct Interface {
    ports: Ports,                              // what an instance shows; each id minted at random
    inputs_node: Ports, outputs_node: Ports,   // built from `ports`, left out of the file and of equality
}

// graph: one graph, its nodes and the nodes it cannot hold
pub struct Graph { nodes: HashMap<NodeId, Node>, pub bindings: BTreeMap<InputPort, Binding>,
                   subscriptions: BTreeSet<Subscription>, stubs: BTreeMap<NodeId, Stub> }
pub struct Stub  { record: DetachedNode, reason: DetachReason }

// graph/subgraph.rs: plain data, no setters, derived serde and equality
pub struct Subgraph { name: String, description: Option<String>, interface: Interface, graph: Graph }

// graph/node
pub(crate) enum NodeKind {
    Body { source: BodySource, cache: CacheMode },   // runs, caches, may subscribe
    Instance(SubgraphId),
    Boundary(Boundary),
}
pub(crate) enum BodySource { Func(FuncId), Special(SpecialNode) }

// graph/node_decl.rs: what a node resolves to
pub enum NodeDecl<'a> {
    Func(&'a Func),
    Instance(&'a Subgraph),
    Boundary { side: Boundary, owner: &'a Subgraph },
}
// ports(), name(), description(), events(), func() -> Option<&Func>, output_is_const(PortId)

// graph/graph_set: every graph of a document, and each rule that lies across them
pub struct GraphSet { main: Graph, subgraphs: BTreeMap<SubgraphId, Subgraph> }

// graph/scope.rs
pub struct Scope<'a> { library: &'a Library, graphs: &'a GraphSet, owner: Option<&'a Subgraph> }
// resolve(&Node) -> Result<NodeDecl<'a>, Unresolved>

// graph/error.rs, graph/reconciled.rs
pub enum Unresolved   { MissingFunc, MissingSubgraph, NoInterface }
pub enum DetachReason { Unresolved(Unresolved), ExtraBoundary, Recursive, TooDeep, TooLarge }
pub enum PlacementRejection { Recursive, TooDeep { graph, levels }, TooLarge { graph, node_count } }
pub struct GraphSetValidationError { graph: GraphId, error: GraphValidationError }
pub struct GraphSetReconciled { main: Reconciled, subgraphs: BTreeMap<SubgraphId, Reconciled> }

// graph/interface_change.rs: the reversible record of an interface edit
pub struct InterfaceChange {
    id: SubgraphId,
    interface: [Interface; 2],            // before, after
    dropped: Vec<DroppedBinding>,         // each wire or constant the new interface does not hold
}
pub struct DroppedBinding { graph: GraphId, port: InputPort, binding: Binding }
```

What `GraphSet` answers, and nothing else does:

| Question | Method |
|---|---|
| a graph, to read or to edit | `graph(GraphId)`, `graph_mut(GraphId)` |
| a definition | `subgraph(id)`, `subgraphs()`, `insert_subgraph`, `remove_subgraph` (unused only), `rename_subgraph` |
| what a graph's nodes resolve against | `scope(library, GraphId)` |
| the graph an instance path ends in | `graph_at(InstancePath) -> Option<GraphId>` |
| who places a subgraph | `uses(id) -> impl Iterator<Item = NodeAddr>` |
| may these nodes be added to this graph | `admit_nodes(GraphId, nodes) -> Result<(), PlacementRejection>` |
| what an interface edit changes | `plan_interface(id, Interface, library) -> InterfaceChange` |
| land one half of it | `write_interface(&InterfaceChange, half)` |
| is this a document a host may compile | `validate_with(library)` |
| make a decoded one so | `reconcile(library) -> GraphSetReconciled` |
| how one definition differs from another | `digest(id) -> SubgraphDigest` |

### Darkroom

```rust
// core/document
pub(crate) struct Document {
    pub(crate) graphs: GraphSet,        // every graph, each with its stubs
    pub(crate) main_view: GraphView,
    /// One per subgraph of `graphs`, as a view holds one placement per node and stub.
    pub(crate) sides: BTreeMap<SubgraphId, SubgraphSide>,
    pub(crate) layout: DockState<TabRef>,
    pub(crate) active: GraphId,         // the graph the window's commands act on; view state, not undoable
}
pub(crate) struct SubgraphSide {
    view: GraphView,
    through: Option<InstancePath>,      // the instance its pane shows it through; view state, not undoable
    shared: bool,                       // listed in the palette of whoever has this file in the library
    synced: Option<SubgraphDigest>,     // the digest last shared with the library
}

pub(crate) enum TabRef { Graph(GraphId), Preferences, ImageViewer(ProgramId), Inspector(InspectTarget) }
pub(crate) enum InspectTarget { Selection, Node { addr: NodeAddr, through: Option<InstancePath> } }

// core/edit
pub(crate) enum DocumentRequest {
    Graph { graph: GraphId, intent: GraphIntent },
    Subgraph(SubgraphIntent),
    View(DockOperation<TabRef>),
    ..
}
pub(crate) enum SubgraphIntent { Create, Rename, SetInterface, Collapse, Expand, MakeUnique,
                                 RemoveUnused, Update, .. }
pub(crate) enum UndoStep {
    Graph { graph: GraphId, step: GraphStep },       // today's nine kinds, each against one graph
    Subgraph(Box<SetSubgraph>),                      // a definition and its side, in or out
    Interface(Box<SetInterface>),                    // an InterfaceChange and the routes of what it drops
    SubgraphName(SetSubgraphName),
}
/// One graph of the document and its view, borrowed apart: what a GraphStep writes.
pub(crate) struct GraphSlot<'a> { graph: &'a mut Graph, view: &'a mut GraphView }

pub(crate) struct EditFacts { node_set: bool, renamed: bool, binding_kind: bool, touched: Touched }
pub(crate) enum Touched { Nothing, Graph(GraphId), Several }

// core/edit/fragment.rs: a set of nodes as it moves
pub(crate) struct Fragment {
    nodes: Vec<FragmentNode>,             // id, node, the bindings on its own inputs, placement
    subscriptions: Vec<Subscription>,
    ties: BTreeMap<TieId, Tie>,
    routes: BTreeMap<WireKey, Route>,
}
// capture(&Document, GraphId, items), remap(fresh ids), into_intents(offset, IncomingWires)
pub(crate) struct Clip { fragment: Fragment, subgraphs: BTreeMap<SubgraphId, (Subgraph, GraphView)> }

// gui/state/run_state
nodes:     HashMap<ProgramId, NodeRunState>,
instances: HashMap<InstancePath, InstanceRunState>,   // worst status, summed time, the node that failed

// gui: two contexts where one stood
pub(crate) struct DocCtx<'a>   { app: AppCtx<'a>, open: &'a OpenDocument }
pub(crate) struct GraphCtx<'a> { doc: DocCtx<'a>, graph: GraphId, body: &'a Graph, view: &'a GraphView,
                                 through: Option<InstancePath>, camera: Viewport }

// gui/main_window
graph_panes: HashMap<GraphId, GraphUi>,               // one per open graph tab
image_viewers: HashMap<ProgramId, ImageViewer>,
```

### Dependencies

```
common ← scenarium ← lens ← darkroom ← darkroom-app

scenarium:   data ← graph ← exec ← worker          library reads data and graph::func
graph:       identity, ports
               ← func, interface
               ← subgraph, node
               ← graph (one graph's rules)
               ← graph_set (the rules across graphs), interface_change
             scope and declaration read library and graph_set
darkroom:    core::document ← core::edit ← core::open_document ← gui
             core::runtime_host reads Document.graphs alone
```

The edges from `graph::func` into `runtime` stay, as a lambda's signature names the context it
runs in.

### Ownership

```
App
├─ RuntimeLibrary ── Library (funcs, types, codecs)             fixed for a process
├─ SharedLibrary ── the library folder as last read             replaced whole on a change
└─ Session
   ├─ OpenDocument
   │  ├─ Document
   │  │  ├─ GraphSet ── main: Graph { nodes, bindings, subscriptions, stubs }
   │  │  │           └─ subgraphs: SubgraphId → Subgraph { name, interface, graph }
   │  │  ├─ main_view: GraphView
   │  │  ├─ sides: SubgraphId → SubgraphSide { view, through, shared, synced }
   │  │  ├─ layout
   │  │  └─ active: GraphId
   │  └─ UndoHistory ── UndoStep (owns each InterfaceChange and DetachedNode)
   ├─ RuntimeHost ── Compiler, WorkerBridge ──▶ worker thread: engine, caches
   ├─ RunState ── nodes by ProgramId, instances by InstancePath, PreviewStore by ProgramId
   └─ MainWindow ── one GraphUi per open GraphId, image viewers by ProgramId
```

Everything borrowed is borrowed downward for a call: a `Scope` and a `NodeDecl` from the
`GraphSet` and the `Library`, a `GraphSlot` from the `Document`, a `DocCtx` from the session
for one phase of a frame, and a `GraphCtx` from that for one pane.

Two tables are keyed alike and kept in step by the edits, as a view's placements are with a
graph's nodes: `GraphSet.subgraphs` and `Document.sides`. One step, `SetSubgraph`, moves an
entry of both. Refused: the view inside scenarium's `Subgraph`, which puts the editor's layout
in the framework; a `GraphSet` generic over what a host keeps beside a subgraph, which threads
a type parameter through the scope and the compile.

Shared ownership, each case:

| Case | Verdict |
|---|---|
| `DataType::FsPath(Arc<FsPathConfig>)` | Replaced by a file kind's id and a mode, both `Copy`; the list is the registry's, written once. |
| `PathPick.config: Arc<FsPathConfig>` | Replaced by the `Copy` type; the dialog reads the list from the registry. |
| `static SUBGRAPHS` in `core/main_graph.rs` | Deleted with the file; the document owns its graphs. |
| `FuncLambda(Arc<AsyncLambda>)`, `EventLambda(Arc<dyn Fn>)` | Kept. A compiled program outlives a recompose of the library that made it, on another thread. |
| `CompiledGraph.codecs: Arc<Codecs>` | Kept, for the same reason. |
| `Arc<CompiledGraph>` | Kept. The host, the worker and the run state each hold it for a time none of them decides. |
| `PreviewSink.latest: Mutex<HashMap<..>>` | Kept. The worker writes and the frame drains. Its key becomes `ProgramId`. |
| `MoveSelection.members: Rc<[DragStart]>` | Kept. One latched list read by every frame of a drag. Never serialized. |

### Call graphs

Load:

```
OpenDocument::load
└─ io::document::read ─▶ Document (decoded, unchecked)
└─ Document::reconcile(library)
   ├─ GraphSet::reconcile(library)
   │  ├─ per graph: Graph::resolving_stubs(scope) ─▶ attach, with the wiring that still fits
   │  ├─ per graph: Graph::unresolved(scope) ─▶ detach into its stubs    read, then write
   │  ├─ walk: the instances that close a loop ─▶ detach
   │  ├─ in post-order: the nodes past a bound ─▶ detach
   │  └─ per graph: Graph::refused_wiring(scope) ─▶ remove; closing wires ─▶ remove
   ├─ one SubgraphSide per subgraph; repair each view against its graph's nodes and stubs
   └─ repair the layout, each `through` path and `active` against the graphs
```

A frame:

```
Session::frame
├─ DocCtx { app, open }
├─ MainWindow::input
│  └─ per graph tab on screen: GraphCtx::of(doc, graph) ─▶ graph_panes[graph].input
│     the pane that takes input asks to be the active graph
├─ drain the requests into the document
└─ MainWindow::record
   ├─ menu bar, status bar, the selection's inspector ─▶ GraphCtx::of(doc, document.active)
   └─ per graph tab: its own GraphCtx ─▶ graph_panes[graph].record
```

Edit:

```
widget ─▶ DocumentQueue::push_graph(graph, intent)
OpenDocument::drain_requests
└─ GraphIntent::commit(document, graph, library, steps)
   ├─ into_step(&Document, graph)                         read only
   │  ├─ validate::*, Graph::admit(scope, ..), Graph::produces_cycle
   │  └─ GraphSet::admit_nodes(graph, ..)                 recursion and both bounds, every graph
   └─ UndoStep::apply(&mut Document)
      └─ GraphStep::write(document, graph, dir) ─▶ Document::slot_mut(graph)
└─ UndoHistory::push ─▶ EditFacts { .., touched } ─▶ Session ─▶ the panes of the graphs touched

Duplicate, Paste, Expand:
  Fragment::capture(&Document, graph, items) ─▶ remap(fresh ids) ─▶ into_intents ─▶ the queue above
```

Interface edit:

```
inspector, or a wire dropped on a boundary node ─▶ SubgraphIntent::SetInterface { id, to }
└─ into_step: GraphSet::plan_interface(id, to, library) ─▶ InterfaceChange    read only
              + the route of each wire it drops, per graph
└─ SetInterface::write(document, dir)
   ├─ GraphSet::write_interface(&change, half)   the interface, and each dropped binding out or back
   └─ the routes of the dropped wires, in each view that held one
```

Compile, run, report:

```
Session ─▶ RuntimeHost::run_once(&GraphSet)
└─ Compiler::compile(&GraphSet, &Library)
   ├─ GraphSet::validate_with(library)                    the one release assert
   ├─ place_nodes: expand each instance by InstancePath; note each subgraph's Outputs node
   ├─ per placed node: Scope::resolve ─▶ NodeDecl::Func ─▶ ExecNode; each port id ─▶ its position
   │                   lower(binding): across each boundary to a func's output, a constant or nothing
   └─ validate_debug
└─ WorkerBridge::install(Arc<CompiledGraph>) ─▶ worker

worker ─▶ WorkerReport { program_id, .. } ─▶ RunState::apply_report
          nodes[program_id]; folded once into instances[each prefix of its path]
preview lambda ─▶ PreviewSink::publish(ctx.cur_program_id(), value) ─▶ PreviewStore

pane of graph G through path P:
  a node N reads      run_state.node(P.node(N))
  an instance I reads run_state.instance(P.child(I))
```
