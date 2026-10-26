# The editor harness

A way for an agent, or a test, to run the whole editor with no desktop: open a project, click and
type in its UI, and read back what it shows as text and as pixels. An agent reaches it as an MCP
server. The plan is `HARNESS_PLAN.md`.

## Decisions

- **The harness drives the real `App`.** It drives the runtime host, the worker, the preview
  converter and every pane, so a project runs and its previews appear as in the window. Refused:
  `SessionHarness`, which has no runtime and no previews, and a fixture library, which cannot open
  a real project.
- **The core is in darkroom, the MCP server is its own crate.** The core drives darkroom's private
  `App`, so it is `darkroom::internals::harness`, tested on its Rust API. The server is the member
  crate `mcp/` (package `darkroom-mcp`, `publish = false`, one binary), which maps tools onto the
  core, so darkroom's own build never compiles the MCP SDK. Refused: the whole harness in a new
  crate, which would make `App` and its internals public; the server in darkroom behind a feature,
  which `--all-features` would build on every check.
- **An agent reaches it through MCP over stdio.** Claude Code starts the server from the project's
  `.mcp.json` and keeps it for the agent's session, so a project opened once is clicked many
  times, and a shot comes back as an image the agent sees at once. Refused: a loopback server with
  a `send` client, which an agent must start and address by hand; scripts only, which replay the
  whole session for each step ([Claude Code MCP](https://code.claude.com/docs/en/mcp)).
- **`.mcp.json` starts a built binary.** It runs `target/mcp/darkroom-mcp`, which `cargo drive run
  mcp` builds under the `mcp` profile (`release-dev` that unwinds a panic), and `darkroom_open`
  replies with the commit the binary was built from and whether the tree differed, so a stale
  server says so. Refused: `cargo run` in `.mcp.json`, whose cold build outlasts the client's
  startup timeout and waits on any other build's lock.
- **The server uses rmcp.** rmcp is the official Rust SDK of the protocol: it keeps the protocol
  version, the capability negotiation, the schemas (from schemars) and the content types in step
  with the spec. Refused: our own JSON-RPC loop, whose protocol and schemas would be ours to keep
  in step with a spec that changes each year
  ([rust-sdk](https://github.com/modelcontextprotocol/rust-sdk)).
- **Six consolidated tools.** `darkroom_open`, `darkroom_act` (a batch of actions, optionally
  ending in a shot), `darkroom_state`, `darkroom_ui`, `darkroom_shot`, `darkroom_wait_run`. Names
  come before ids in what they take and return, `darkroom_state` has a concise and a detailed
  form, and an error says how to correct the call. This follows Anthropic's guidance for agent
  tools: few tools for whole workflows, one prefix, readable names, a response-format switch,
  errors that can be acted on. Refused: one tool per action, which makes a four-step gesture four
  results in the agent's context; tools that edit through `GraphIntent`, which skip the UI under
  test ([Writing tools for agents](https://www.anthropic.com/engineering/writing-tools-for-agents)).
- **Every reply fits the client's budget.** Claude Code warns at 10,000 tokens of tool output and
  cuts at 25,000. A concise state of the sample stays under 2,000 tokens; `darkroom_ui` takes a
  filter and a limit, 50 by default, and says how many it left out. Refused: whole dumps that the
  client truncates where it chooses.
- **Locators by meaning, then by text, then by position.** darkroom's own locators name what the
  document holds: a node by name or id, a port by node, side and name, a tab, a world point on the
  canvas. A text locator finds a widget by the text it shows. A screen point is the last resort.
  This is the order that Playwright and egui_kittest recommend. A locator that matches more than
  one widget is an error that lists the matches, as Playwright's strict locators do
  ([Playwright locators](https://playwright.dev/docs/locators),
  [egui_kittest](https://docs.rs/egui_kittest)).
- **palantir lists what it drew.** Under palantir's `internals`, the `Ui` lists the widgets of the
  last frame that sense input: the id, the screen rect, what it senses, and the text of its
  subtree, nested sensing widgets left out, so a button's label belongs to the button. Refused:
  AccessKit, a dependency and a role and label for every widget kind, a much larger palantir change
  than the harness needs.
- **Each action waits until the editor settles.** After its input, an action runs frames until a
  frame asks no repaint, no input is held, and the app has no wake pending, bounded by a timeout.
  A timer wake (`repaint_after`, as a caret blink asks) does not hold it, or a focused text field
  would never settle. `darkroom_wait_run` waits for the worker too. This is Playwright's
  auto-wait. Refused: sleeps and frame counts, which break when a frame takes longer.
- **Text state first, pixels when the look matters.** `darkroom_state` and `darkroom_ui` reply with
  text, with the status log's and the tracing log's warnings and errors since the last call.
  `darkroom_shot` replies with a PNG of a region (the window, a pane, a node with a margin, or a
  world rect), scaled so that its longer side is at most `max_side` (1280 by default), with a
  legend of the visible nodes and ports at their pixel rects in that image, so a point seen in the
  picture becomes a locator. An image costs about w·h/750 tokens, and Playwright MCP gives agents
  an accessibility snapshot before a screenshot for the same reason
  ([Playwright MCP](https://github.com/microsoft/playwright-mcp)).
- **A screenshot is product API, and the harness takes its shots through it.** palantir's `Ui`
  asks for a capture of the frame it records, and the host hands the image over a frame later, as
  egui's `ViewportCommand::Screenshot` does; darkroom's View menu copies or saves the view with it.
  The harness's `darkroom_shot` asks the same way, so an agent sees what a user's capture gives.
  Refused: a test-only readback for the harness beside a product capture, two paths to keep right;
  `Gpu::read_back` stays for palantir's visual suite
  ([egui ViewportCommand](https://docs.rs/egui/latest/egui/viewport/enum.ViewportCommand.html)).
- **The display is chosen when the harness is built.** `Display::Gpu` renders each frame into an
  offscreen texture, and a build with no GPU adapter panics. `Display::Cpu` runs the same frames on
  palantir's deviceless frontend, where `darkroom_shot` is an error. Both shape text with the
  bundled fonts, so a locator resolves to the same rect on both. The harness's own tests use
  `Cpu`, and one test uses `Gpu`. Refused: a silent fallback from one to the other, which turns a
  missing adapter into a test that passes without having run.
- **The harness steps its own clock.** Each frame advances the UI clock by 1/60 s, and separate
  clicks are spaced past the double-click window, so animation and click timing repeat from run
  to run. The worker runs on real time, which a run needs.
- **A panic ends the session, not the server.** A logic error still panics, as everywhere in the
  editor; the server catches it at the harness thread's edge, drops the session, and answers with
  the panic message and its location, so the agent can report it and open again. Refused: a
  server process that dies with the editor, which the client shows only as a lost connection.
- **A session works on a copy of the project's folder, in RAM.** The harness copies the
  document's folder into its session directory and opens the copy, so relative paths, saves and the
  runtime cache stay inside the session. The session directory is under the system's temporary
  directory (`common::env::temp_dir`), which is tmpfs on Debian 13 and Arch: the copy, the cache and
  the shots cost no disk writes and go with a reboot. A folder over 256 MiB, or a document whose
  relative paths leave its folder, is refused with the reason, as the copy would fill RAM or
  break the paths. `darkroom_open { in_place: true }` opens a project where it is, warm cache
  included, when the user asks for it. Cost: lumos's frame store refuses to spill into a
  memory-backed directory, so a project whose run spills opens `in_place`.
- **A sample project lives in the repo.** `darkroom/sample/` holds `sample.darkroom`, in the
  editor's own RON text, and the small inputs it reads by relative path. It is made and changed in
  the editor, through the harness or by hand, never generated. A test opens it with no load
  repair and runs it, so a format change that breaks it fails in the same commit.
- **A document holds relative and absolute paths.** A relative path resolves against the
  document's folder at run time, and an absolute path is used as written, as Blender's `//` paths
  are. A preference, "store picked paths relative", on by default, makes a file picked inside the
  document's folder a relative path; an unsaved document has no folder, so its picks are
  absolute. Save As to another folder rewrites the relative paths so that they keep naming the
  same files ([Blender relative paths](https://docs.blender.org/manual/en/latest/files/blend/open_save.html)).
- **Native dialogs are a seam.** `App` asks for a path through a `Dialogs` value: native in the
  window, scripted in the harness, where `darkroom_act` answers each dialog in turn and an
  unanswered one is an error. A blocking native dialog would hang a session that no one can see.
