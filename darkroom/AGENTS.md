# Darkroom

The editor: a native desktop app on palantir over the `darkroom` library, and the harness that
runs it with no desktop for an agent or a test.

## The editor harness

`darkroom::internals::harness` drives the whole editor in a window no one sees: it opens a
project, plays input into it, waits for it to settle, and reads back what it shows as text and as
pixels. The member crate `mcp/` (`darkroom-mcp`) serves it to an agent.

- **The harness drives the real `App`**, with its runtime host, worker, preview converter and
  every pane, so a project runs and its previews appear as in the window. Refused:
  `SessionHarness`, which has no runtime and no previews, and a fixture library, which cannot open
  a real project.
- **The core is in darkroom, the MCP server is its own crate.** The core drives darkroom's private
  `App`; the server only maps tools onto it, so darkroom's own build never compiles the MCP SDK.
  Refused: the whole harness in a new crate, which would make `App` public; the server in darkroom
  behind a feature, which `--all-features` would build on every check.
- **An agent reaches it through MCP over stdio**, which Claude Code starts from `.mcp.json` and
  keeps for the agent's session, so a project opened once is clicked many times and a shot comes
  back as an image the agent sees. Refused: a loopback server an agent must start and address by
  hand; scripts, which replay the whole session for each step
  ([Claude Code MCP](https://code.claude.com/docs/en/mcp)).
- **`.mcp.json` starts a built binary**, `target/mcp/darkroom-mcp`, which `cargo drive run mcp`
  builds under the `mcp` profile: `release-dev` that unwinds a panic, as `release` aborts and Cargo
  sets `panic` per profile only. `darkroom_open` replies with the commit the binary was built from
  and whether the tree differed, so a stale server says so. Refused: `cargo run` in `.mcp.json`,
  whose cold build outlasts the client's startup timeout.
- **The server uses rmcp**, the protocol's official Rust SDK, which keeps the protocol version,
  the schemas and the content types in step with the spec. Refused: our own JSON-RPC loop
  ([rust-sdk](https://github.com/modelcontextprotocol/rust-sdk)). The tools' JSON shapes are the
  server's own types, converted to the harness's, so the harness carries no schema.
- **Six consolidated tools**: `darkroom_open`, `darkroom_act` (a batch of actions, played one at
  a time so a failure names the action it stopped at, optionally ending in a shot),
  `darkroom_state`, `darkroom_ui`, `darkroom_shot`, `darkroom_wait_run`. Names come before ids,
  `darkroom_state` has a concise and a detailed form, and an error says how to correct the call.
  Refused: one tool per action, which makes a four-step gesture four results; tools that edit
  through `GraphIntent`, which skip the UI under test
  ([Writing tools for agents](https://www.anthropic.com/engineering/writing-tools-for-agents)).
- **Every reply fits the client's budget**: Claude Code warns at 10,000 tokens of tool output and
  cuts at 25,000, so `darkroom_ui` takes a filter and a limit, 50 by default, and says how many it
  left out. Refused: whole dumps that the client truncates where it chooses.
- **Locators by meaning, then by text, then by position**: a node by name or id, a port by node,
  side and name, a world point, then a widget's text, then a screen point. A locator that matches
  several is an error that lists them, and `Nth` picks one, as Playwright's strict locators and
  `nth` do ([Playwright locators](https://playwright.dev/docs/locators)).
- **palantir lists what it drew**: under its `internals`, `Ui::inspect` lists the last frame's
  widgets that sense input, with their rects and the text of their subtree. Refused: AccessKit, a
  role and a label for every widget kind, a much larger palantir change than the harness needs.
- **Each action waits until the editor settles**: frames run until one asks no repaint, the app
  has no wake pending, and no preview is converting on its thread, bounded in frames and in real
  time; a timer wake, as a caret blink asks, does not hold it. A wake from the worker or the
  preview converter makes the next frame a full one, as the window host's repaint does: a frame
  nothing asked for paints only and runs no `update`. `darkroom_wait_run` waits for the
  worker too. This is Playwright's auto-wait. Refused: a fixed sleep or frame count, which breaks
  when a frame or a conversion takes longer.
- **Text first, pixels when the look matters.** `darkroom_state` and `darkroom_ui` reply with
  text and the warnings and errors logged since the last call. `darkroom_shot` replies with a PNG
  scaled to `max_side` (1280 by default) and a legend of each visible node and port at its rect in
  the picture, so a point seen in it becomes a locator. An image costs about w·h/750 tokens, which
  is why Playwright MCP gives agents an accessibility snapshot first
  ([Playwright MCP](https://github.com/microsoft/playwright-mcp)).
- **A shot scales down in linear light** with a tent filter widened by the ratio, as the frame
  blends. Refused: a mean of the encoded bytes, which darkens every edge between light and dark.
- **A screenshot is product API, and the harness shoots through it**: View ▸ Copy View as Image
  and Save View as PNG… take the window through `Ui::request_screenshot`, and `darkroom_shot`
  asks the same way, so an agent sees what a user's capture gives, as egui's
  `ViewportCommand::Screenshot` does. Refused: a test-only readback beside a product capture, two
  paths to keep right
  ([egui ViewportCommand](https://docs.rs/egui/latest/egui/viewport/enum.ViewportCommand.html)).
- **The display is chosen when the harness is built**: `Gpu` renders into an offscreen texture
  and panics with no adapter; `Cpu` runs the same frames on palantir's deviceless frontend, where
  a shot is an error. Both shape text with the bundled fonts, so a locator resolves to the same
  rect on both. Refused: a silent fallback, which turns a missing adapter into a test that passes
  without having run.
- **The harness steps its own clock**, 1/60 s a frame, and spaces separate clicks past the
  double-click window, so animation and click timing repeat from run to run. The worker runs on
  real time, which a run needs.
- **A panic ends the session, not the server.** The server catches it at the editor thread's
  edge, drops the session, and answers with the panic's message and location. Refused: a server
  that dies with the editor, which the client shows only as a lost connection.
- **A quit ends the session**, as it closes the window: once an action quits the editor (File ▸
  Quit, or Ctrl+Q past the unsaved-changes prompt), every harness call that runs a frame is
  refused with `HarnessError::Quit`, and the MCP server closes the session and says so. Refused: a
  session that goes on after its editor asked to end, which a user never sees.
- **A session works on a copy of the project's folder, in RAM**, under `common::env::temp_dir`,
  tmpfs on Debian 13 and Arch, so saves, the runtime cache and the shots stay inside the session
  and cost no disk writes. A folder over 256 MiB, one holding a link to a folder, or a document
  whose relative paths climb out of its folder is refused with the reason; `in_place` opens the
  project where it is. Cost: lumos's frame store refuses to spill into a memory-backed directory,
  so a project whose run spills opens in place.
- **Native dialogs are a seam.** `App` asks for a path through a `Dialogs` value: native in the
  window, scripted in the harness, where each ask takes the next answer, made absolute as a native
  dialog's is. A blocking native dialog would hang a session no one can see.
- **A sample project lives in the repo**, `darkroom/sample/`: `sample.darkroom` and the small
  inputs it reads by relative path. It is made and changed in the editor, through the harness or
  by hand, never generated, and a test opens it with no load repair and runs it to its previews,
  so a change that breaks it fails in the same commit.

## The document file

- **A `.darkroom` file is the document's RON text, or that text zipped**, told apart by the
  file's first bytes, so both share the extension and its file association. A save keeps the
  format the file was opened in; Save As… writes plain RON, and Save As Compressed… writes it
  zipped, a fifth of the size. Plain text is what a person, an agent and git read; the ZIP is for
  size.

## Paths in a document

- **The file holds the paths inside its folder relative; memory holds every path absolute.** A
  load joins each relative path to the file's folder, part by part, so the separators are the
  system's own; a save writes each path inside the document's folder relative to it, `/` between
  its parts, and never with `..`, so the folder moves or is copied whole. The preference "Save
  paths inside the project folder as relative", on by default, turns it off. Refused: relative
  paths in memory, resolved at run time — the undo history holds the path values of earlier edits,
  so after Save As to another folder an undo would put back a path that names another file, and
  every path reader in `lens` and the worker would need the folder. Blender keeps its `//` paths
  relative in memory, so its Save As must remap them
  ([Blender relative paths](https://docs.blender.org/manual/en/latest/files/blend/open_save.html)).
- **A path is made absolute where it enters memory**: a load resolves the file's, a dialog answers
  absolute, and the default model paths, bare file names, are taken against the working directory
  when the library is composed. `Document::validate`, which every load and save runs in a debug
  build, refuses a relative path, so a new way in that skips this is loud.

## Wire ties

A tie gathers wires into a ribbon on the graph canvas, as a cable tie gathers cables. It is
layout only: the graph does not change, and the run does not read it.

- **A tie is view data.** Ties and routes live in `GraphView` beside the node placements: they are
  saved, undoable, and ignored by compile, plan and run. A load repairs them as it repairs the rest
  of the view, and every edit that ends a wire carries its route. Refused: a pass-through node, as
  Substance Designer's Cable Tie node does, which changes the graph for a layout need; scenarium has
  no design for reroute nodes ([Cable Tie](https://jclark.gumroad.com/l/EZAgl)).
- **A route is stored, a strand's slot is derived.** Each tied wire stores its ordered route of tie
  ids. Each frame, a strand's slot comes from the barycenter of its two neighbouring points along
  the band axis, which is the crossing-reduction heuristic of layered graph drawing (Sugiyama,
  Tagawa, Toda 1981). At creation this is the order the line crossed the wires. Refused: a stored
  order, as Zip Ties stores one, because strands can cross inside the band after nodes move.
- **The tie line is Shift+RMB-drag, and Ctrl+Shift+LMB-drag on a trackpad.** Blender draws a line
  across links with Shift+RMB to add reroutes, beside its Ctrl+RMB link cut. darkroom's breaker is
  RMB and Ctrl+LMB, so the tie line is the breaker's chord plus Shift. Refused: hold Z, as ComfyUI
  Zip Ties does, because palantir has no key-release event; Alt+LMB, because Alt-drag moves windows
  on some Linux desktops and Blender uses it to detach links
  ([Blender Reroute](https://docs.blender.org/manual/en/latest/interface/controls/nodes/types/layout/reroute.html),
  [KWin MR 12](https://invent.kde.org/plasma/kwin/-/merge_requests/12)).
- **The line only ties.** A straight line from the press to the pointer, whose crossings the wire
  and band draws record as the breaker's do; it ties every crossed wire into one new tie at the mean
  of the crossings. A line that crosses a tie adds the other crossed wires to that tie. Delete or
  the tie's menu removes a tie. Refused: a line that removes the tie it crosses, as both ComfyUI
  extensions do, which gives one gesture two opposite results and no way to add a wire.
- **A tie holds wires of both families.** Data and event wires mix in one tie, as the breaker cuts
  both. Refused: data wires only, and one tie for each family.
- **A wire is both of its ends.** A route is keyed by `WireKey`, `Data(DataWire { output, input })`
  or `Event(Subscription)`, so a rewire makes another key and validation catches a route an edit
  forgot. Rewiring either end, disconnecting or removing a node drops the wire's route in the same
  undo step, and a tie with no wires goes with it. Refused: a route keyed by the input alone, which
  puts a strand from an unrelated output into a bundle and which a load cannot check. Zip Ties
  follows the same rule.
- **A tie is an item.** `GraphView::selected` holds `ItemId`, `Node(NodeId)` or `Tie(TieId)`. Band
  select, click, Shift-click, Delete, Esc and the group drag treat a tie as they treat a node, as
  Blender and Unreal treat reroute points. Refused: a second selection set kept in step by hand, and
  ties that cannot be selected.
- **A node set takes the ties inside it.** A drag or a duplicate of a node set also takes each tie
  whose every wire has both end nodes in the set, plus the selected ties for a drag. One rule for
  move and duplicate, as Zip Ties uses for its moves and copies. Refused: Blender's rule, where only
  selected items move and a ribbon stretches when its nodes are clicked and dragged.
- **The band turns itself, and a knob pins it.** An unpinned band stands square to the mean
  direction of its strands. Dragging the knob of a selected tie pins the angle: free, or in 15°
  steps with Shift, as design tools snap. A double-click on the knob frees it
  ([comfyui-cabletie](https://github.com/nathancoooper/comfyui-cabletie)).
- **One wire leaves through the tie's menu.** RMB-click on a tie opens the anchored menu: a Release
  entry for each wire, which lights its strand under the pointer, Pin or Free angle, and Remove tie.
  Refused: dragging a strand out of the band, a target 2.5 units wide that would need wire
  hit-testing, which the canvas avoids; a Release submenu, which palantir's menus lack.
- **A tied wire is one cubic per piece.** The pieces run from port to tie, tie to tie, and tie to
  port, with the tangent along the strands at each tie, so the curve stays G1. An untied wire keeps
  `Wire::data` and `Wire::event`. The colour ramp splits at the piece ends by chord length through
  `ColorRamp::color_at`, so a tied wire takes the Oklab colours an untied one does. Refused: a
  linear ramp for tied wires, another colour space at the same two ends.
- **The band is neutral and paints over wires, under nodes.** The band takes the muted ink and lifts
  on hover and selection, so the wire colours, which are the types, stay clear. It belongs to the
  plumbing, so a node body covers it as it covers wires. Refused: a colour for each tie.
- **No motion of its own.** A band follows the pointer and the nodes directly. Refused: the spring
  motion of comfyui-cabletie, which lags the pointer and repaints the canvas until it settles.
