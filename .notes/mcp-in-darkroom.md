# MCP in the darkroom binary

Proposal: one `darkroom` binary whose command line picks a window or no window, and MCP or no
MCP. It replaces the separate `darkroom-mcp` binary and answers "make darkroom start mcp and not
otherwise" in `.notes/TODO.md`. When it lands, its decisions fold into `darkroom/AGENTS.md`, "The
editor harness", which some of them change, and this file is deleted.

## Decisions

- **Flags on one binary.** `darkroom [FILE]` is the window, as now. `darkroom --mcp [FILE]` is the
  window and an MCP server on stdio, which Claude Code starts from `.mcp.json`: the person sees
  the window the agent drives and can take over at any time. `darkroom --mcp --headless
  [--display gpu|cpu] [--size WxH] [--scale S]` is no window, as `darkroom-mcp` runs now.
  `--headless` requires `--mcp`, as a run with no window and no driver has nothing to do; a batch
  mode can come later as its own flag. Playwright MCP and Chrome DevTools MCP take the same shape:
  one server a client launches over stdio, a window by default, `--headless` for none
  ([Playwright MCP](https://github.com/microsoft/playwright-mcp),
  [Chrome DevTools MCP](https://raw.githubusercontent.com/ChromeDevTools/chrome-devtools-mcp/main/docs/configuration.md)).
  Refused: a `darkroom mcp` subcommand, one more level for the same two choices; keeping
  `darkroom-mcp` beside `darkroom`, two binaries that drift and a window no agent drives. Blender
  and Godot pair an in-app socket with a separate stdio bridge only because they cannot change the
  host app; darkroom serves in-process.
- **The binary is its own crate.** `darkroom` becomes a library. A crate `app/` (`darkroom-app`)
  takes over `mcp/` and builds the `darkroom` binary: the CLI, the window or headless start, and
  the MCP server. Checks and tests of the library never build rmcp. Refused: the server in the
  library, which every darkroom test build then compiles; the server behind a library feature,
  which `--all-features` builds on every lint.
- **stdio only, for now.** Claude Code starts the editor itself; no port listens. HTTP waits for
  a need, as `--mcp-port PORT`: the spec then asks the server to check `Origin`, bind to
  127.0.0.1 and authenticate, and rmcp's HTTP server brings an HTTP stack
  ([MCP transports](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)).
  Refused: HTTP only, as Figma's desktop app serves, where Claude Code cannot start the editor and
  the connection fails until a person does.
- **Nothing but MCP on stdout.** The stdio transport forbids any other byte there and allows logs
  on stderr ([MCP transports](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)),
  so `darkroom::init_tracing`, which writes to stdout now, writes to stderr in every mode. A
  release build on Windows is a GUI-subsystem binary, which inherits no console but takes the
  pipes a parent hands it, as Claude Code does
  ([rust#67159](https://github.com/rust-lang/rust/issues/67159)).
- **One driver, two backends.** Actions, locators, state, shots and the settle rules are one
  driver over an `App`. Offscreen: today's harness, the app on its own thread, a stepped clock, a
  `HeadlessWindow`. Window: the app on the winit main thread, each tool call carried to it by
  `HostHandle::run_on_main` and answered on a oneshot, real time. A shot uses the product
  screenshot API in both. The driver leaves `internals` for product code; its fixtures stay.
- **palantir puts input into a window as the OS does.** Two `HostHandle` calls, public palantir
  API: `send_input(window, OwnedInputEvent)`, which the host queues and delivers before the next
  frame as it delivers OS input, with its clock and its repaint; and `last_frame(window)`, the
  window's last `FrameReport`, so the driver waits for a frame that asks no repaint. Test
  frameworks inject at this layer, past the platform: Qt's QTest creates events "on the spot",
  egui_kittest feeds simulated events to its harness
  ([qtestmouse.h](https://code.qt.io/cgit/qt/qtbase.git/tree/src/testlib/qtestmouse.h?h=5.14),
  [kittest](https://docs.rs/kittest)). Refused: lending the `Ui` and making `Ui::on_input`
  public, which skips the host's clock and repaint, as QTest's `sendEvent` skips logic a real
  click runs; a window the agent only watches.
- **The process is the session.** `darkroom_open` opens a project in the running app; a quit ends
  the process, and the client sees the server end. Refused: sessions that start and stop inside
  one server, which a window cannot do.
- **A dialog takes a queued answer, else asks the person.** An agent's `answer` action queues an
  answer; the next ask takes one if there is one, else opens the native dialog. A person's own
  Save As… works as always, and the driver reports an action waiting on a native dialog rather
  than hanging. Refused: all dialogs scripted under `--mcp`, which cancels the person's own;
  native only, where an agent cannot save or open through the menus.
- **A project opens in place.** In both modes `darkroom_open` opens the file where it is, as a
  person's Open does; `copy` asks for the RAM copy of its folder. This changes "A session works
  on a copy of the project's folder, in RAM" in `darkroom/AGENTS.md`. Refused: a copy by
  default, whose saves a person watching the window would have to find in a temp folder; a
  default that differs by mode.

## palantir first

The two `HostHandle` calls are a palantir change of their own, proposed and landed before the
window backend.
