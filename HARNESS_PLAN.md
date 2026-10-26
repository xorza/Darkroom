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
- `darkroom::internals::harness::Harness` runs the real `App` on `DisplayKind::Gpu` (palantir's
  `internals::HeadlessWindow`) or `DisplayKind::Cpu` (`UiHarness`): `open` (a copy of the folder,
  or in place), `resize`, `act(&[Action])`, `state(Detail, nodes)`, `ui(filter, limit)`,
  `wait_run(timeout)`, with `Locator`s by node, title, port, world point, text, screen point,
  offset and `Nth`.
- `App::new(ui, AppHost, LaunchInputs)`: `AppHost::Offscreen(Arc<OffscreenSignals>)` gives the
  harness the app's wakes and quit; `LaunchInputs` carries a `PreferencesStore` (`File(path)` or
  `Memory`) and a `Dialogs` (`Native`, or `Scripted` answers under `internals`).
- A path const is used as written: `Load Image` does `PathBuf::from(inputs[0].required_fs_path())`,
  so a relative path resolves against the process's working directory.

## Plan

### 1. darkroom: copy and save the view

Files: `darkroom/src/gui/frame/app_command.rs`, the View menu, `darkroom/src/gui/app/mod.rs`.

- [ ] View ▸ Copy view as image and View ▸ Save view as PNG…, through `AppCommand`: the command
  asks `Ui::request_screenshot(None)`, and the frame whose `Ui::screenshot_taken()` holds the
  image copies it to the clipboard or saves it through `Dialogs` and `common::fs`. Test: a
  scripted Save answers a path, and the file there decodes to the screenshot's pixels.

### 2. Shots

Files: `darkroom/src/harness/display.rs`, `darkroom/src/harness/shot.rs`,
`darkroom/src/harness/tests.rs`.

- [ ] `Harness::shot(region: Region, max_side: u32) -> Shot`: asks `Ui::request_screenshot` for
  the region and runs frames until `Ui::screenshot_taken()` holds it, so the agent sees what a user's capture gives;
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

### 3. The MCP server

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

### 4. Relative paths in a document

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

### 5. The sample project

Files: new `darkroom/sample/` (`sample.darkroom`, `inputs/`), its test in
`darkroom/src/core/open_document/tests.rs`.

- [ ] Made through the harness with `in_place`: a new document saved as
  `darkroom/sample/sample.darkroom`; Load Image of `inputs/lena.tiff` (a copy of imaginarium's
  fixture) through a few image nodes to two previews, and Load Astro Image of
  `inputs/full_example.fits` (lumos's fixture) to a third, laid out on screen at the default
  viewport; then a shot to check the look. Ids come from the editor, so they are unique.
- [ ] Test: the sample opens with an empty `LoadReport`, compiles and runs to completion on the
  real library, and each preview receives an image of the input's size.

### 6. Fold the decisions into the documents

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
  through palantir's screenshot API; `internals` now enables `image`.
- `InspectedWidget` also carries `focusable` and `disabled`, which the hit table holds and an agent
  needs to tell a dead control from a live one.
- palantir's screenshot landed as `Ui::request_screenshot(Option<Rect>)` and the frame edge
  `Ui::screenshot_taken() -> Option<&Screenshot>`, not `screenshot()`: an image belongs to the one
  frame it lands in, as a click does. It has no `ScreenshotError::Unsupported`: a frame that asks
  paints in full through the backbuffer, which every target has, GLES included, and a blit into
  `Rgba8UnormSrgb` gives BGRA and float targets the same bytes.
- The seams: `PreferencesStore::File` holds its path, and loading goes through the store too, so
  a test points a session at a temp file; `Dialogs` is a value `LaunchInputs` carries. A scripted
  ask with no answer of its shape reads as a cancel and logs an error, which the harness's log
  reports, rather than writing the status log: the dialogs know no status log, and a pick already
  returns `None` for a cancel.
- The harness core: the session directory is under `common::env::temp_dir()` as planned. These
  differ from the plan, each for the reason given:
  - `Locator::Nth(locator, n)` picks one of several `Node` or `Text` matches, as Playwright's
    `nth` does: a palette's search field shows the typed text beside the row it finds.
  - No `Tab` locator: a tab chip shows its name, so `Text` finds it.
  - `resize` takes a size only; the scale is the session's, as a window keeps its monitor.
  - `State` reports `can_undo`, not an undo depth, which the history does not count; and no open
    popups, which `Ui::inspect` does not mark: their rows are in `ui()`.
  - The relative-path check of a copy waits for the batch that brings relative paths.
  - The log ring is the harness thread's default subscriber, so it holds the app's own warnings
    and errors, not the worker thread's.
  - palantir gained `internals::HeadlessWindow`, so darkroom names no wgpu type, and
    `internals::DOUBLE_CLICK_WINDOW`, so the click spacing is the input state's own.
