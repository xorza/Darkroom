# The editor harness

The harness as `.notes/editor-harness.md` decides it. `WIRE_TIES_PLAN.md` starts after this plan
lands, and it checks each of its batches with the harness.

Each batch leaves `cargo drive check` and `cargo drive test` green. palantir is its own repository:
its change is committed there, and the submodule pointer moves in the same darkroom commit that
uses it. The new dependency, rmcp 3.7 (`server`, `macros`, `transport-io`, and `client` for its
tests), is approved for `darkroom-mcp` only.

The harness comes first, on `DocumentFixture` documents. The sample project comes last: it needs
relative paths, and it is made in the editor through the finished harness.

## What exists

- palantir's `OffscreenHost` renders an `App` into a texture with no window, and takes an injected
  `Clock`. Under palantir's `internals`, `Gpu::read_back(&texture)` copies an 8-bit RGBA or BGRA
  texture back as an `RgbaImage` in RGBA order, and `Ui::inspect(&mut out)` lists the last frame's
  interactive widgets as `InspectedWidget { id, rect, sense, focusable, disabled, clipped, text }`.
- palantir's `UiHarness` drives a `Ui` with synthetic input on the deviceless frontend
  (`on_input`, `press_at`, `drag_to`, `key`, `type_text`, `set_modifiers`, `rect(id)`,
  `hit_at(pos)`, `advance`, `frame_app`), with real shaping through `with_text`.
- `FrameReport` says `repaint_requested` and `repaint_after`; a focused text field's caret asks
  `repaint_after` for 30 s.
- darkroom's `SessionHarness` and `CanvasHarness` run on fixtures, with no runtime and no worker.
- `App::new` takes a `HostHandle<App>`, which only `WinitHost` makes. `App` saves preferences
  unless `preferences_writable` is false. File dialogs are blocking `rfd` calls in
  `gui/dialogs.rs`.
- A path const is used as written: `Load Image` does `PathBuf::from(inputs[0].required_fs_path())`,
  so a relative path resolves against the process's working directory.

## Plan

### 1. darkroom: the seams the harness plugs into

Files: `darkroom/src/gui/app/mod.rs`, `darkroom/src/lib.rs`, `darkroom/src/gui/dialogs.rs`,
`darkroom/src/core/io/preferences/mod.rs`.

- [ ] `App` holds `AppHost`, `Window(HostHandle<App>)` or `Offscreen(OffscreenSignals)`, with
  `wake()` and `quit()`. `OffscreenSignals` holds two atomics that the harness reads: a wake
  pending, and a quit asked. `run` builds `Window`.
- [ ] `PreferencesStore`, `File` or `Memory`, replaces `preferences_writable`: `save_preferences`
  writes only for `File`. `LaunchInputs` carries it; `run` passes `File` when the load succeeded,
  and the harness passes `Memory`.
- [ ] `Dialogs`, `Native` or `Scripted(VecDeque<DialogAnswer>)`: each `dialogs::pick_*` call in
  `App` goes through it. `Scripted` pops the next answer; an ask with no answer left is an error
  in the status log, not a block. Test: a scripted Open dialog opens the answered path.

### 2. The harness core

Files: new `darkroom/src/harness/` (`mod.rs`, `display.rs`, `action.rs`, `locator.rs`,
`state.rs`, `clock.rs`, `log.rs`, `tests.rs`); `darkroom/src/lib.rs`'s `internals`.

- [ ] `Harness::new(HarnessConfig { display: DisplayKind, size: UVec2, scale: f32 })` makes the
  session directory `darkroom-harness-<pid>-<n>` under `common::env::temp_dir()`, removed when the
  harness drops, and builds `App` with `AppHost::Offscreen`, `PreferencesStore::Memory` and
  `Dialogs::Scripted`. `DisplayKind::Gpu` opens `RequestedGpu::headless` and an `OffscreenHost`
  with bundled fonts, and panics with the adapter error when there is none; `DisplayKind::Cpu`
  runs `UiHarness::with_text` and `frame_app`. Both take input as `InputEvent`s through
  `on_input`, so one input path serves both.
- [ ] `Harness::open(path, in_place: bool)`: copies the document's folder into the session
  directory and opens the copy, or opens the path itself with `in_place`. A folder over 256 MiB, or
  a relative path that leaves the folder, is `HarnessError::CopyRefused` with the reason and the
  `in_place` hint. `Harness::resize(size, scale)`.
- [ ] `SteppedClock`: each frame advances 1/60 s, injected into `OffscreenHost` and driven by
  `UiHarness::advance` on `Cpu`; a click after another click advances past `DOUBLE_CLICK_WINDOW`
  first, unless the action is a double-click.
- [ ] `Action`: `Move(Locator)`, `Click { at, button, mods }`, `DoubleClick(at)`,
  `Drag { from, to, button, mods, steps }`, `Scroll { at, delta }`, `Pinch { at, factor }`,
  `Key(chord)` (`"ctrl+z"`), `Type(text)`, `Answer(DialogAnswer)`. `Harness::act(&[Action])`
  runs each and settles after each: frames until a frame asks no repaint, no input is held and no
  app wake is pending, a timer wake not counted, bounded by a timeout that is an error naming the
  step. `act` returns what changed: the selection, dirty, focus and popups.
- [ ] `Locator`: `Node(name or id)`, `NodeHeader(..)`, `Port { node, side, port }`, `Tab(name)`,
  `World(x, y)`, `Text(string)`, `Screen(x, y)`, each with an optional offset. Node and port
  locators resolve through the graph pane's geometry and the viewport; `Text` through
  `Ui::inspect`. More than one match is `HarnessError::Ambiguous` with every match; none is
  `NotFound` with the nearest names.
- [ ] `Harness::state(Detail)`: `Concise` gives the nodes by name with their screen rects, the
  selection, dirty, focus, open popups, the status line, and the warnings and errors logged since
  the last call; `Detailed` adds ids, ports with their bindings, the wires, the viewport, undo depth
  and the run status of each node, and takes a node-name filter. `Harness::ui(filter, limit)`
  gives `Ui::inspect`'s unclipped entries and the count it left out; each entry's text is cut at
  120 characters with its full length, as a scroll area's text is all its rows.
  `Harness::wait_run(timeout)`. The log is a tracing layer the harness installs, kept in a ring
  of 200 lines.
- [ ] Tests with `DisplayKind::Cpu` on `DocumentFixture` documents saved to a temp folder: a click
  on a node selects it; a drag of a node header moves it by the pointer's travel, exactly; a drag
  from an output port to an input port binds the wire; `Key("ctrl+z")` takes it back; a
  right-click on the canvas opens the new-node popup, `Type` filters it, and a click on a `Text`
  locator adds the node; `Node("missing")` is `NotFound`, and two nodes of one name are
  `Ambiguous`; a focused text field still settles; a copied session leaves the source folder
  unchanged byte for byte; a 257 MiB folder is refused (made of one sparse file).

### 3. palantir: a screenshot of the frame

Files: palantir's `ui/` (the request and the delivery), `host/window_driver/`, `host/winit/`,
`host/offscreen.rs`, `gpu/` (the copy), `lib.rs` (the public surface), `.notes/API_SURFACE.md`;
darkroom's `gui/frame/app_command.rs` and the View menu.

- [ ] Public API, approved: `Ui::request_screenshot(region: Option<Rect>)` asks for the frame
  being recorded, the region in logical px (`None` for the whole window). The host copies that
  frame as it presents it, and the next frame's `Ui::screenshot() -> Option<Screenshot>` hands it
  over once. `Screenshot` holds the RGBA8 pixels, their physical size and the scale, as a type
  of palantir's own, so the public API does not expose the `image` crate. This is the shape of
  egui's `ViewportCommand::Screenshot`: ask in one frame, receive in a later one
  ([egui ViewportCommand](https://docs.rs/egui/latest/egui/viewport/enum.ViewportCommand.html)).
- [ ] The copy comes from the backbuffer the frame painted, so a damaged-only frame still yields
  the whole picture; on a target with no backbuffer and no `COPY_SRC` (a GLES swapchain), the
  request answers `ScreenshotError::Unsupported` instead of an image. `OffscreenHost` and
  `WinitHost` share the one path in `WindowDriver`.
- [ ] Tests: an offscreen frame of a known scene asked for a screenshot gives, one frame later,
  the same pixels as `Gpu::read_back` of the target; a region gives that crop, rounded out to
  whole physical pixels at scale 2; a second `screenshot()` call gives `None`; a frame that skipped
  paint still gives the whole picture. `cargo drive surface` regenerates `.notes/API_SURFACE.md`.
- [ ] darkroom: View ▸ Copy view as image and View ▸ Save view as PNG…, through
  `AppCommand`; the save goes through `Dialogs` and `common::fs`.

### 4. Shots

Files: `darkroom/src/harness/display.rs`, `darkroom/src/harness/shot.rs`,
`darkroom/src/harness/tests.rs`.

- [ ] `Harness::shot(region: Region, max_side: u32) -> Shot`: asks `Ui::request_screenshot` for
  the region and runs the frame that delivers it, so the agent sees what a user's capture gives;
  the region is `Window`, `Pane(name)`, `Node(name)` with a 24 px margin, or `World(rect)`,
  resolved to logical px. It scales the image so that its longer side is at most `max_side`, and
  writes `<session>/shots/<n>-<name>.png` through `common::fs`. `Shot` holds the PNG bytes, the
  path and the legend: each visible node and port with its rect in the image's pixels. On `Cpu` it
  is `HarnessError::NoGpu`. `act` takes an optional shot to end with.
- [ ] Test with `DisplayKind::Gpu`: a 400 × 300 frame of one node at a known place; the pixel at
  (2, 2) is the palette's `canvas_bg` and the pixel at the node body's centre is `node_fill`,
  exactly, both read from `Palette`. A region of one node is the node's screen rect plus the
  margin; a `max_side` of 100 scales a 400 × 300 window to 100 × 75 and its legend by 1/4. A node's
  rect from `state` is the same on `Cpu` and `Gpu`.

### 5. The MCP server

Files: new `mcp/` (`Cargo.toml`, `build.rs`, `src/main.rs`, `src/tools.rs`, `src/tests.rs`), root
`Cargo.toml` (member, dependency, drive command), new `.mcp.json`.

- [ ] `darkroom-mcp`, `publish = false`, depending on `darkroom` with `internals` and on rmcp. One
  binary: `darkroom-mcp [--display gpu|cpu] [--size WxH] [--scale S]`, default `gpu`, serving over
  stdio. The harness lives on one thread, started through `common::thread`; the tools reach it
  through a channel. A panic on that thread drops the session, and the next call gets the panic's
  message and location; `darkroom_open` starts a new one. `build.rs` records the commit and
  whether the tree was dirty.
- [ ] The tools, each with a description written for an agent and an example input:
  `darkroom_open { project, in_place, size, scale }` (replies with the build's commit, the
  document's summary and the session directory), `darkroom_act { actions, shot }`,
  `darkroom_state { detail, nodes }`, `darkroom_ui { filter, limit }`,
  `darkroom_shot { region, max_side }` (an image block, the path and the legend),
  `darkroom_wait_run { timeout_s }`. A `HarnessError` becomes a tool error that says how to fix
  the call, with the matches for `Ambiguous` and the nearest names for `NotFound`.
- [ ] Root `Cargo.toml`: a drive command `mcp`, `build -p darkroom-mcp --profile release-dev`.
  `.mcp.json` at the root: the server `darkroom`, command `target/release-dev/darkroom-mcp`.
- [ ] Tests through rmcp's in-process client: `tools/list` names the six tools with their
  schemas; `darkroom_open` of a fixture then `darkroom_state` lists its nodes; an `Ambiguous`
  locator is a tool error that lists both matches; a harness panic answers the next call with its
  message and leaves the server serving.

### 6. Relative paths in a document

Files: `lens/src/image/nodes/io.rs`, `lens/src/astro/nodes/io.rs` and every other reader of
`required_fs_path`; scenarium's invocation context; `darkroom/src/core/runtime_host/`,
`darkroom/src/core/io/preferences/`, `darkroom/src/gui/pane/preferences.rs`,
`darkroom/src/gui/app/mod.rs` (the path picks and Save As).

- [ ] The invocation context carries the document's folder (`None` for an unsaved document).
  Each path input resolves through one function: a relative path joins the folder, an absolute
  path stays. A relative path in an unsaved document is an `InvokeError` that says to save first.
  Test: `a/b.tiff` in a document at `/p/x.darkroom` reads `/p/a/b.tiff`; `/q/c.tiff` reads
  `/q/c.tiff`; `a/b.tiff` in an unsaved document is the error.
- [ ] `Preferences::store_relative_paths`, default true, with a row in the preferences pane. A
  pick inside the document's folder stores the path relative to it when the preference is on.
  Test: the four cases of on/off by inside/outside, and an unsaved document storing absolute.
- [ ] Save As to another folder rewrites each relative path so that it names the same file, in
  the same undo step as the save's path change. Test: `a/b.tiff` saved from `/p` to `/p/q` becomes
  `../a/b.tiff`.

### 7. The sample project

Files: new `darkroom/sample/` (`sample.darkroom`, `inputs/`), its test in
`darkroom/src/core/open_document/tests.rs`.

- [ ] Made through the harness with `in_place`: a new document saved as
  `darkroom/sample/sample.darkroom`; Load Image of `inputs/lena.tiff` (a copy of imaginarium's
  fixture) through a few image nodes to two previews, and Load Astro Image of
  `inputs/full_example.fits` (lumos's fixture) to a third, laid out on screen at the default
  viewport; then a shot to check the look. Ids come from the editor, so they are unique.
- [ ] Test: the sample opens with an empty `LoadReport`, compiles and runs to completion on the
  real library, and each preview receives an image of the input's size.

### 8. Fold the decisions into the documents

Files: new `darkroom/AGENTS.md`, root `AGENTS.md`, `.notes/editor-harness.md`, `HARNESS_PLAN.md`.

- [ ] New `darkroom/AGENTS.md`, "The editor harness" and "Paths in a document": each decision,
  with what it refuses and its source. Root `AGENTS.md`: the crate list adds `darkroom-mcp` and
  says that `darkroom/AGENTS.md` holds the editor's decisions, as it says for `simd`;
  Verification says how an agent builds the server, opens the sample and takes a shot, in three
  lines.
- [ ] Delete `.notes/editor-harness.md` and `HARNESS_PLAN.md`.

## Changed

- palantir's readback is `Gpu::read_back` under `internals`, not `golden::read_back`: wgpu types
  stay in `crate::gpu`, and its callers are palantir's visual suite and tests, as the harness shoots
  through the screenshot API of batch 3; `internals` now enables `image`.
- `InspectedWidget` also carries `focusable` and `disabled`, which the hit table holds and an agent
  needs to tell a dead control from a live one.
