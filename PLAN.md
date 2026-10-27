# MCP in the darkroom binary

The work `.notes/mcp-in-darkroom.md` decides. palantir lands first, as the window backend needs
its two calls. Each batch leaves `cargo drive check` and `cargo drive test` green, in darkroom and
in palantir.

## Open before batch 1

The driver becomes product code, but it uses four items of palantir's `internals` feature, test
code: `HeadlessWindow` (the offscreen backend), `UiHarness` (the `cpu` display),
`Ui::inspect` with `InspectedWidget` (locators and the widget list), and `DOUBLE_CLICK_WINDOW`.
The product binary must not turn on `internals`, so each one either becomes public palantir API
or leaves the product. This is a public palantir API decision, still yours to take; batch 1
assumes the first three become public and the `cpu` display stays test-only, so `--display` goes
from the command line.

## Plan

### 1. palantir: input into a window, and its last frame

Files: palantir `src/host/winit/handle.rs`, `src/host/winit/mod.rs`, `src/host/winit/window/`,
`src/input/` (the owned event), `.notes/API_SURFACE.md`.

- [ ] `OwnedInputEvent`: `InputEvent` with its borrowed IME text owned, built `From` an
  `InputEvent` and lent back with `as_event()`, so an event crosses threads.
- [ ] `HostHandle::send_input(window, OwnedInputEvent)`: a `UserEvent::Input` the event loop
  delivers to that window through the path OS input takes, `ui.on_input(event, now)` with the
  host's clock, then the repaint it asks. Events keep their order among themselves and with OS
  input. A closed window drops it.
- [ ] `HostHandle::last_frame(window) -> Option<FrameSummary>`: the window's last frame, its
  number, `repaint_requested` and `repaint_after`, written by the event loop after each frame into
  a slot the handle reads from any thread. `None` before the first frame and after the window
  closes. `FrameSummary` is `Copy` and `Send`; `FrameReport` keeps its crate-private fields.
- [ ] As the open decision above settles: `HeadlessWindow`, `Ui::inspect` with
  `InspectedWidget`, and the double-click window leave `internals` for public API.
- [ ] Tests, on the winit host's own test setup: an event sent before a frame reaches the `Ui`
  in that frame, as the same OS event does; a press and a release sent in order click a button;
  `last_frame` advances by one each frame and reports a frame that asked a repaint; both calls on
  a closed window do nothing. Regenerate the API surface.

### 2. Logs on stderr

Files: `darkroom/src/lib.rs`.

- [ ] `init_tracing` writes to stderr (`.with_writer(io::stderr)`), in every mode, so a log line
  never reaches an MCP stream on stdout.

### 3. The `darkroom` binary becomes the crate `app/`

Files: new `app/` (`Cargo.toml`, `build.rs`, `src/main.rs`, `src/cli.rs`, `src/mcp/` from
`mcp/src/`); `darkroom/Cargo.toml`, `darkroom/src/main.rs` (removed); root `Cargo.toml` (members,
`default-members`, the drive table's `mcp` command); `.mcp.json`; `darkroom/packaging/linux/`,
`darkroom/packaging/macos/`; `AGENTS.md`.

- [ ] `darkroom` is a library only. `app/` (`darkroom-app`, binary `darkroom`) holds `main.rs`,
  the CLI, and the MCP server, moved from `mcp/`, which goes. It takes `mcp/`'s build script
  (the commit a build reports) and the `mcp` profile.
- [ ] The CLI: `darkroom [FILE]`, `--mcp`, and `--headless` with `--size WxH` and `--scale S`,
  which require `--mcp` (clap `requires`). Without `--mcp` it runs the window
  as `darkroom::run` does now. Test: the parse table, each refused combination included.
- [ ] `--mcp --headless` runs today's server over the offscreen harness, one session per
  process: the session starts at launch on `FILE`, or on a new document; `darkroom_open` opens
  another document in it; a quit ends the process. The display, size and scale come from the
  command line. The `cpu` display stays a test backend under `internals`. Test: the server tests
  from `mcp/`, moved and changed to one session.
- [ ] `.mcp.json` starts `target/mcp/darkroom` with `["--mcp", "--headless"]`. The drive `mcp`
  command builds `-p darkroom-app --profile mcp`; packaging builds `-p darkroom-app`;
  `default-members` is `app`.

### 4. The driver is product code, with the dialog queue and opening in place

Files: `darkroom/src/internals/harness/` → `darkroom/src/driver/` (all but `fixture.rs`, which
stays in `internals`); `darkroom/src/gui/dialogs.rs`; `app/src/mcp/`; `darkroom/AGENTS.md`'s
cited paths.

- [ ] Move the harness out of `internals` into `darkroom::driver`: `Driver` with the actions,
  locators, state, shots and settle rules, over a `Backend`, `Offscreen` today's `Display` and
  stepped clock. Every gate that only the harness needed goes.
- [ ] `Dialogs` holds a queue of answers in front of the native dialog: an ask takes the next
  queued answer of its shape, else asks the backend's fallback, the native dialog in a window,
  a cancel and an error headless. Test: a queued answer wins, an empty queue falls back, and a
  queued answer of the wrong shape is an error, as now.
- [ ] `darkroom_open` opens in place; `copy` asks for the RAM copy of the folder, with today's
  refusals. Test: the open tests, with `copy` where they relied on the copy.

### 5. The window backend

Files: `darkroom/src/driver/` (the window backend), `darkroom/src/gui/app/` (the request queue
an `App` serves), `app/src/main.rs`, `app/src/mcp/`.

- [ ] `darkroom --mcp`: the winit host runs the `App` on the main thread; the MCP server runs on
  its own thread. A tool call becomes requests: input goes through `HostHandle::send_input`; what
  needs the `Ui` (state, the widget list, a shot) goes into a queue, pushed through `run_on_main`,
  that the `App` serves in its next frame where palantir lends the `Ui`, and its answer comes back
  on a oneshot.
- [ ] Settle on real time: after an action, wait for a frame newer than the last request whose
  summary asks no repaint, with no wake pending and no preview converting, bounded in time.
- [ ] A native dialog blocks the main thread: the `App` raises a flag before it opens one, and
  a call that waits on it answers "waiting on a native dialog" rather than timing out silently.
- [ ] A quit closes the window and ends the process; the server ends with it.
- [ ] Tests: the `App`'s request queue served in a frame, on the offscreen host; the settle
  rule against a sequence of frame summaries. Check by hand: Claude Code starts `darkroom --mcp`
  from `.mcp.json` with `["--mcp"]`, opens the sample, clicks a node, and takes a shot, in a
  window you watch.

### 6. Fold the decisions into the documents

Files: `darkroom/AGENTS.md`, `AGENTS.md`, `.notes/TODO.md`, `.notes/mcp-in-darkroom.md`,
`PLAN.md`.

- [ ] `darkroom/AGENTS.md`, "The editor harness": each decision of `.notes/mcp-in-darkroom.md`,
  replacing the ones it changes (the crate split, the RAM copy, the server binary).
- [ ] `AGENTS.md`'s crate list and verification lines name `darkroom-app` and `darkroom --mcp`.
- [ ] Delete the "make darkroom start mcp and not otherwise" item in `.notes/TODO.md`, then
  delete `.notes/mcp-in-darkroom.md` and `PLAN.md`.
