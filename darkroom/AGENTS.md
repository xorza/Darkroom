# Darkroom

The editor: a native desktop app on palantir over the `darkroom` library, and the harness that
runs it with no desktop for an agent or a test.

## The editor harness

`darkroom::driver` drives the whole editor as a person at its window would: it opens a project,
plays input into the window, waits for it to settle, and reads back what it shows as text and as
pixels. The window is offscreen, or the desktop one. The `darkroom` binary of `app/`
(`darkroom-app`) serves it to an agent with `--mcp`.

- **Flags on one binary.** `darkroom [FILE]` is the window. `darkroom --mcp [FILE]` is the window
  and an MCP server on stdio: the person sees the window the agent drives and can take over at any
  time. `darkroom --mcp --headless [--size WxH] [--scale S]` is no window. `--headless` requires
  `--mcp`, as a run with no window and no driver has nothing to do. Playwright MCP and Chrome
  DevTools MCP take the same shape
  ([Playwright MCP](https://github.com/microsoft/playwright-mcp),
  [Chrome DevTools MCP](https://raw.githubusercontent.com/ChromeDevTools/chrome-devtools-mcp/main/docs/configuration.md)).
  Refused: a `darkroom mcp` subcommand, one more level for the same two choices; a separate
  server binary beside the editor, two binaries that drift and a window no agent drives.
- **The binary is its own crate.** `darkroom` is a library; `app/` builds the binary: the CLI,
  the window or headless start, and the MCP server. Checks and tests of the library never build
  rmcp. Refused: the server in the library, which every darkroom test build then compiles; the
  server behind a library feature, which `--all-features` builds on every lint.
- **The driver is product code, and palantir publishes what it reads.** The binary does not build
  `internals`: palantir's `HeadlessWindow`, `Ui::inspect` with `InspectedWidget`, and
  `Ui::double_click_window` are public API. iced and egui ship a headless runner as product API
  ([iced_test](https://docs.rs/iced_test), [egui_kittest](https://docs.rs/egui_kittest)), and
  iced_test selects a widget by its text as `inspect` does. The double-click window is a query, as
  Qt's `QStyleHints::mouseDoubleClickInterval` and egui's `InputOptions::max_double_click_delay`
  are, so it can follow the OS setting with no change to callers. Refused: an AccessKit tree for
  locators, as egui_kittest reads, which waits for palantir to take on screen readers.
- **One driver, two backends.** Actions, locators, state, shots and the settle rules are one
  driver over a backend. Offscreen: the app on the driver's thread, a stepped clock, a
  `HeadlessWindow`. Window: the app on the winit main thread; input goes in through
  `HostHandle::send_input`, and what needs the `Ui` is a request the app serves in its next frame,
  answered on a channel. A shot uses the product screenshot API in both, and an open the app's
  driven open: File ▸ Open, with no prompt.
- **palantir puts input into a window as the OS does.** `HostHandle::send_input` delivers an
  event before the next frame as OS input is, with the window's input clock, then runs a frame;
  `HostHandle::last_frame` reads the window's last frame summary, which counts the sent events
  before it. Test frameworks inject at this layer, past the platform: Qt's QTest creates events
  "on the spot", egui_kittest feeds simulated events to its harness
  ([qtestmouse.h](https://code.qt.io/cgit/qt/qtbase.git/tree/src/testlib/qtestmouse.h?h=5.14),
  [kittest](https://docs.rs/kittest)). Refused: lending the `Ui` and making `Ui::on_input`
  public, which skips the host's clock and repaint, as QTest's `sendEvent` skips logic a real
  click runs.
- **The harness drives the real `App`**, with its runtime host, worker, preview converter and
  every pane, so a project runs and its previews appear as in the window. Refused:
  `SessionHarness`, which has no runtime and no previews, and a fixture library, which cannot open
  a real project.
- **An agent reaches it through MCP over stdio**, which Claude Code starts from `.mcp.json` and
  keeps for the agent's session, so a project opened once is clicked many times and a shot comes
  back as an image the agent sees. No port listens: HTTP waits for a need, when the spec asks the
  server to check `Origin`, bind to 127.0.0.1 and authenticate
  ([MCP transports](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)).
  Refused: a loopback server an agent must start and address by hand; HTTP only, as Figma's
  desktop app serves, where the client cannot start the editor; scripts, which replay the whole
  session for each step ([Claude Code MCP](https://code.claude.com/docs/en/mcp)).
- **Nothing but MCP on stdout.** The stdio transport forbids any other byte there and allows logs
  on stderr, so `init_tracing` writes to stderr in every mode. A release build on Windows is a
  GUI-subsystem binary, which takes the pipes a parent hands it
  ([rust#67159](https://github.com/rust-lang/rust/issues/67159)).
- **`.mcp.json` starts a built binary**, `target/mcp/darkroom --mcp --headless`, which
  `cargo drive run mcp` builds under the `mcp` profile: `release-dev` that unwinds a panic, as
  `release` aborts and Cargo sets `panic` per profile only. `darkroom_open` replies with the
  commit the binary was built from and whether the tree differed, so a stale server says so.
  Refused: `cargo run` in `.mcp.json`, whose cold build outlasts the client's startup timeout.
- **The server uses rmcp**, the protocol's official Rust SDK, which keeps the protocol version,
  the schemas and the content types in step with the spec. Refused: our own JSON-RPC loop
  ([rust-sdk](https://github.com/modelcontextprotocol/rust-sdk)). The tools' JSON shapes are the
  server's own types, converted to the driver's, so the driver carries no schema.
- **Six consolidated tools**: `darkroom_open`, `darkroom_act` (a batch of actions, checked whole
  and then played in order, a failure naming the action it stopped at, optionally ending in a
  shot whose failure keeps the actions' report),
  `darkroom_state`, `darkroom_ui`, `darkroom_shot`, `darkroom_wait_run`. Names come before ids,
  `darkroom_state` has a concise and a detailed form, and an error says how to correct the call.
  Refused: one tool per action, which makes a four-step gesture four results; tools that edit
  through `GraphIntent`, which skip the UI under test
  ([Writing tools for agents](https://www.anthropic.com/engineering/writing-tools-for-agents)).
- **Every reply fits the client's budget**: Claude Code warns at 10,000 tokens of tool output and
  cuts at 25,000, so `darkroom_ui` takes a filter and a limit, 50 by default, `darkroom_state`
  a node filter and a cap of 100 nodes, and each says how many it left out. Refused: whole dumps that the client truncates where it chooses.
- **Locators by meaning, then by text, then by position**: a node by name or id, a port by node,
  side and name, a world point, then a widget's text, then a screen point. A locator that matches
  several is an error that lists them, and `Nth` picks one, as Playwright's strict locators and
  `nth` do ([Playwright locators](https://playwright.dev/docs/locators)).
- **Each action waits until the editor settles**, as Playwright's auto-wait does. Offscreen,
  frames run until one asks no repaint, the app has no wake pending, and no preview is converting
  on its thread, bounded in frames and in real time; a wake from the worker or the preview
  converter makes the next frame a full one, as the window host's repaint does. In the window, the
  settle waits for a frame that saw every sent event and asks no repaint, then for no preview
  converting, bounded in real time. A timer wake, as a caret blink asks, holds neither. A shot's
  image is copied back by the device: offscreen, the driver waits for the device, and the next
  frame lands the image; in the window, the wait is bounded in real time.
  `darkroom_wait_run` waits for the worker too. Refused: a fixed sleep or frame count, which
  breaks when a frame, a conversion or a slow adapter takes longer, as a software adapter's copy
  outlasted 600 offscreen frames on the CI runners.
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
  `ViewportCommand::Screenshot` does. Each ask gets a `ScreenshotId`, and
  `Ui::screenshot_taken` reads the image by it, so a person's capture and a shot that ask
  together each take their own. Refused: a test-only readback beside a product capture, two
  paths to keep right
  ([egui ViewportCommand](https://docs.rs/egui/latest/egui/viewport/enum.ViewportCommand.html)).
- **No `cpu` display in the product.** `--headless` needs a GPU adapter, and a software one
  (lavapipe, llvmpipe, WARP) counts, as a headless browser renders in software. With none, the
  start fails with the adapter error. The `cpu` display, palantir's deviceless frontend, where a
  shot is an error, stays a test backend. Both shape text with the bundled fonts, so a locator
  resolves to the same rect on both. Refused: `--display cpu`, which makes palantir's test harness
  public API for a mode with no shots; a silent fallback, which turns a missing adapter into a test
  that passes without having run.
- **The offscreen harness steps its own time**, `OffscreenTime::Manual` advanced 1/60 s a frame,
  and spaces each press past the double-click window from the last one, whatever actions came
  between them, so animation and click timing repeat
  from run to run. The worker runs on real time, which a run needs; the window runs on real time
  throughout.
- **The process is the session.** `darkroom_open` opens a project in the running editor, in place
  of the open one; one that does not open leaves the editor as it was. A quit (File ▸ Quit, or
  Ctrl+Q past the unsaved-changes prompt) ends the process, and the client sees the server end. A
  panic ends the session: the server answers with the panic's message and location, then ends.
  With a window, the window stays when the server ends, for the person at it, and a panic in a
  request the window serves ends that request alone, its message passed to the session. Refused: sessions
  that start and stop inside one server, which a window cannot do.
- **A project opens in place.** `darkroom_open` opens the file where it is, as a person's Open
  does; `copy` opens a copy of its folder under `common::env::temp_dir`, tmpfs on Debian 13 and
  Arch, so saves, the runtime cache and the shots stay inside the session. A copy refuses a folder
  over 256 MiB, one holding a link to a folder, or a document whose relative paths climb out of
  its folder, with the reason. In the window, an open refuses to discard unsaved edits, which the
  person may own; offscreen, where no person edits, it discards them. Refused: a copy by default, whose saves a person watching the window would have
  to find in a temp folder; a default that differs by mode.
- **A dialog takes a queued answer, else asks the person.** `App` asks for a path through a
  `Dialogs` value: an agent's `answer` action queues an answer, made absolute as a native dialog's
  is, and the next ask takes it; with none queued, the window opens the native dialog and the
  headless editor refuses. A native dialog blocks the main thread, so a call waiting on the window
  says so rather than timing out, as it does for a minimized or occluded window, which
  `HostHandle::is_hidden` reports. Refused: all dialogs scripted under `--mcp`, which cancels the
  person's own; native only, where an agent cannot save or open through the menus.
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

## Undo

- **The camera is outside the undo history.** Pan and zoom are saved with the document and do
  not dirty it, but Ctrl+Z never moves them, so a view move never stands between the person and
  their last edit, and a zoom during a drag never splits the drag's entry. Refused: an entry per
  camera gesture, and one merged into the drag. Photoshop keeps zoom and scroll out of History
  because they change no pixels
  ([Adobe](https://helpx.adobe.com/photoshop-elements/desktop/workspace-and-environment/undo-redo-cancel-actions.html)),
  and Blender records a view move only when it moves the camera object
  ([T92099](https://lists.blender.org/pipermail/bf-blender-cvs/2022-August/176504.html)).
- **A held gesture is one entry, and Esc takes it back whole.** What a gesture does joins its
  entry, a preview spawn and the drag that places it alike, and Esc reverts that entry and drops
  it, so a cancelled spawn leaves no card and no entry. Refused: Blender's Shift+D rule, where Esc
  cancels only the move and the copy stays
  ([manual](https://docs.blender.org/manual/en/latest/modeling/meshes/editing/mesh/duplicate.html)),
  which here leaves a card on its producer's port.

## Paths in a document

- **The file holds the paths inside its folder relative; memory holds every path absolute.** A
  load joins each relative path to the file's folder, part by part, so the separators are the
  system's own; a save writes each path inside the document's folder relative to it, `/` between
  its parts, and never with `..`, so the folder moves or is copied whole; a path that reaches
  its place through `..` stays absolute, as only the file system knows where `..` leads past a
  link. The preference "Save
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

## The look

- **A good run is quiet.** A node that ran shows a check and its time in its footer, and keeps
  its resting shadow. Refused: a green glow on each node that ran, which lit the whole canvas
  after every good run; a glow that fades, an animation state for no information.
- **One red frame per cause.** The node whose lambda failed takes the error frame and its first
  error line. A node `RunError::SkippedUpstream` stopped loses its frame, and its footer says
  "Skipped · input failed" in the muted ink. Refused: a red frame on each skipped node, which
  made one missing file into 15 frames of one weight.
- **A status and a type share a hue, never a form.** A status is a node's frame, its footer
  word and the status bar; a type is a port dot and a wire. Bool is the error red, String the
  success green, Path the info blue and Image the warning orange, as Blender and Unreal draw a
  red Bool beside a red error. An event port is white, as Unreal Blueprint draws its execution
  pins, and an untyped input a hollow muted ring, as Blender 5 tells socket structure by
  shape. Refused: moving the four types to their light-row tints, which fall near the 3:1 floor
  and under the 0.06 OKLab wire floor; moving the statuses, which costs the desktop's own alarm
  red and green. Chosen by the user on 2026-10-27.
- **A kind mark is not a status.** One mark after a node's title says what it is, a sink or
  recomputed every run, in the title's white. Refused: the sink mark in the error red.
- **Every port and wire hue clears 3:1 and APCA Lc 40 on a node's body**, the WCAG 2.2 bar for
  a graphic a user must see (SC 1.4.11) and the floor ayu-graphite holds its `light` row to;
  `build.py` checks both beside its wire separation floor. Refused: Lc 45, APCA's figure for
  icons, which no free hue past cyan reaches.
- **A node keeps the `elem` fill**, and a 1 px rule divides its title from its ports. Refused:
  the `panel` fill, which breaks the generator's surface ladder.
- **One type ladder for darkroom and palantir: title 16, body 14, label 12**, the Windows Fluent
  2 ramp. `TypeScale` sets palantir's ambient text, so no palantir widget shows a size of its
  own. Refused: 15/13/11, the macOS and KDE body size, which read small on a 57" screen at scale
  1.5; an 8.5 px caption, too small at scale 1; a label a point under the body, which reads as
  the same size. Chosen by the user on 2026-10-27.
- **Interface size is a preference**, from 80 to 150 % on palantir's `UserScale` steps, as VS
  Code's zoom and Blender's Resolution Scale scale a whole UI: text, spacing, icons and strokes
  together, and the same on every monitor. Refused: a font size alone, which grows text inside
  chrome that keeps its size; a free number, as each factor re-rasterizes every glyph. Chosen by
  the user on 2026-10-27.
- **Space and radius are scales on the theme**, beside `TypeScale`, and the bridge hands them to
  palantir. A literal size at a call site is a missing token.
- **One icon set.** Every glyph on a node and a toolbar is a stroke `Icon` at one stroke width,
  and an "on" control is a tint behind it. Refused: letters (`R`, `D`, `i`) and font glyphs
  (`■`, `~`, `↻`) beside drawn ones.
- **The inspector is a dock pane at UI scale**, following the selection; a pin opens a tab that
  keeps one node. Refused: a panel on the canvas, which zoomed with the nodes to about 6 px
  text and covered its neighbor, and a Closed → Open → Pinned chip the user must remember.
- **A value reads as a field**, as Blender's and Unreal's node inputs do: a quiet box at rest, the
  value at the left of its column, numbers in the mono face, and "—" in the muted ink for an
  input with nothing in it that could hold a constant. Refused: plain text, which does not say
  it is editable.
- **One formatter for each kind of value**, `ShownValue`, which the node's fields and the
  inspector both read. Refused: the inspector's own `ConstValue` text, which showed `5.0000`
  and "15 paths" beside the node's `5.000` and "15 files".
- **A path shows its file name, cut in the middle**, with palantir's `TextWrap::EllipsisMiddle`
  and the whole path as tooltip, so the kept end holds the extension, as macOS, Qt and Pango
  cut a file name.
- **Menus show their keys** with `MenuItem::shortcut_hint`, from the one `shortcuts` table the
  handlers read. A hint keeps the muted ink, as macOS, Windows and GNOME draw key equivalents;
  a disabled row and its hint take `text_disabled`, the text ink at Fluent's
  `TextFillColorDisabled` alpha, which WCAG 2.2 SC 1.4.3 exempts. Refused: the mono face for
  hints, which no platform menu uses; gesture rows in a menu, which a click could not run.
- **The palette is one list in every state**, as VS Code's command palette and Blender's F3
  search are: with no query the library by category under captions, with one a ranked run,
  each row with its description. Refused: one column a category, 870 px wide over the canvas
  the node goes on. Chosen by the user on 2026-10-27.
- **A canvas menu stays where its first frame landed.** Refused: re-anchoring at the opening
  point each frame, which let the window clamp it again, so the palette's field jumped 400 px
  when a query shrank the list.
- **Undo reads "Undo".** Refused: naming the step, as Qt's `QUndoCommand` and AppKit's
  `setActionName` do from the command that made it: darkroom's commands reach the history as
  batches of intents, and a name read off them misnames a compound command (Duplicate would
  read "Undo Add Node").
