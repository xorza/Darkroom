# Review: darkroom on palantir

Scope: `darkroom/src/gui`, `darkroom/src/driver`, `darkroom/src/platform`, `app/src`, and the palantir
public API they call. The goal: move what darkroom rebuilds into palantir, and make palantir's API
simpler and more convenient to use.

Status: proposal. palantir's `AGENTS.md`, "Public API", asks for a go-ahead before each change to the
surface. Approval of a batch of this plan is that go-ahead for the items in it.

Baseline: `cargo drive test` at the root ran 2382 tests (10 ignored) in 3.8 s, all passed. No test
speed finding.

## Part 1: Proposal

### Root causes

1. **A stock widget fixes its content and its states.** `Button` holds a label only, and a look
   changes only with the four interaction states. So darkroom builds `Chip`, `IconButton`, `Link`
   and the node palette's `Row` from a `Panel` and reads last frame's hover to pick a fill
   (`gui/widgets/chip.rs:88`, `icon_button.rs:78`, `link.rs:49`,
   `gui/pane/graph_ui/gesture/new_node_ui/node_palette.rs:282`).
2. **A `Response` borrows the `Ui` immutably.** An overlay that attaches to a widget needs a
   `ResponseSnapshot` to end that borrow first. darkroom wraps this in `support::tooltip_after` and
   calls `.snapshot()` at 9 sites.
3. **Logic that palantir keeps inside one widget is private.** `DragValue` holds a draft buffer that
   commits on Enter or blur. `Scroll` holds the zoom-about-a-point algebra. `TextEdit` holds its
   caret room and padding. darkroom writes each again (`EditBuffer`, `ZoomRange`, `InlineRename`),
   and `InlineRename` copies a private layout detail of `TextEdit` from its source
   (`gui/widgets/inline_rename/mod.rs:117-124`).
4. **The hosts do not give the same services.** Only the winit host has a cross-thread wake
   (`HostHandle`). `Harness::settle` and `Remote` do not know about the app's background work.
   The OS request to open a file never reaches the app. darkroom adds `AppHost`,
   `OffscreenSignals`, two settle loops, a `catch_unwind` and 164 lines of unsafe Objective-C.
5. **The theme vocabulary is not complete.** `Metrics` has no type ramp and no resting shadow, and
   palantir's tooltip sets 12 px by hand (`palantir/src/theme/tooltip.rs:61`). So darkroom keeps its
   own copies of the steps (`SpaceScale`, `RadiusScale`, `StrokeScale`, `TypeScale`,
   `ElevationScale`) and copies them across in `BridgeRoles::metrics`. `RadiusScale` is the same
   as `RadiusSteps`, field for field.
6. **A popup is placed again in each frame.** The clamp to the window moves an open popup when its
   body changes size. darkroom's `AnchoredMenu` keeps the first place by hand.

### Target design

Each bullet is one decision: the choice, the options refused, and the source.

#### Widgets

- **A button holds content and shows a selected state.** `Button::show_with(self, ui, body)`
  records `body(&mut Ui, &TextStyle) -> R` in place of the label and returns `InnerResponse<R>`.
  The body gets the text style that the state resolved, so an icon takes the ink of the label.
  `Button::selected(bool)` picks `ButtonTheme::selected`, a second `StatefulLook`. darkroom's
  `Chip`, `IconButton`, `Link` and palette `Row` become a `Button` with a `ButtonTheme` that
  darkroom builds once. Refused: `Panel` with a stateful look, which has no activation, focus or
  keys; a toggle button bound to `&mut bool`, because each darkroom toggle shows a state that a
  command changes (run, cache, disable), as `aria-pressed` reports a state. Sources:
  [iced `button(content)`](https://docs.rs/iced/latest/iced/widget/fn.button.html),
  [WAI-ARIA button pattern](https://www.w3.org/WAI/ARIA/apg/patterns/button/),
  [Fluent toggle button](https://learn.microsoft.com/windows/apps/design/controls/buttons).
- **`TextEdit` has a buffered mode and a read-only mode.** `TextEdit::buffered(current)` keeps the
  draft in the state map under its id. It fills the draft from `current` while the field is idle,
  restores it on Esc, and commits on Enter or blur. `TextEditResponse::committed_text:
  Option<String>` holds the draft on the frame it commits with a change, so a copy happens at
  gesture rate. `DragValue` uses the same mode for its editor. `TextEdit::read_only(bool)` shows
  the text and refuses edits. Refused: a buffer of the app's own, which every app writes again
  with the blur frame wrong. Sources:
  [Qt `editingFinished`](https://doc.qt.io/qt-6/qlineedit.html#editingFinished),
  [Qt `readOnly`](https://doc.qt.io/qt-6/qlineedit.html#readOnly-prop).
- **An editable label is a palantir widget.** `EditableLabel::new(text)` shows the text, opens a
  buffered `TextEdit` on a double click, and returns `EditableLabelResponse { response, committed:
  Option<String> }`. Setters: `editable(bool)`, `max_chars(n)`, `style(&TextEditTheme)`. palantir
  owns both halves, so the glyphs cannot move at the swap. Refused: darkroom's `InlineRename`,
  which copies `TextEdit`'s private caret room. Source:
  [GTK4 `EditableLabel`](https://docs.gtk.org/gtk4/class.EditableLabel.html).

#### Hosts

- **Every host gives a cross-thread waker.** `Ui::waker() -> Waker`, `Clone + Send + Sync`, with
  `wake()`. On winit it asks the window for a frame. On a `Harness` it sets a flag, and the next
  frame runs whole. darkroom drops `AppHost` and `OffscreenSignals`, and quits through
  `Ui::close_window`, which both hosts serve. Refused: a `HostHandle` on `Harness`, which also
  carries `run_on_main` and `remote`, two winit ideas. Source:
  [egui `Context::request_repaint`](https://docs.rs/egui/latest/egui/struct.Context.html#method.request_repaint),
  which any thread calls.
- **A settle counts the app's background work.** `App::is_busy(&self) -> bool`, `false` by default,
  says that work the app waits on is in flight and will wake the window. `Harness::settle` waits
  on the waker, in real time up to `HarnessBuilder::background_wait`, while the app is busy.
  The winit host writes `FrameSummary::busy` after each frame. `Remote::settle` becomes public,
  and `Remote` waits on a condition variable that the host signals after each frame, not on a
  2 ms sleep. darkroom drops `Offscreen::settle`, `WindowBackend::settle` and the
  `pending_work` flag. Refused: a flag on `Ui` set in each record, which goes stale when no frame
  records. Sources: darkroom `AGENTS.md`, "Each action waits until the editor settles";
  [Playwright auto-wait](https://playwright.dev/docs/actionability).
- **A remote query that panics returns an error.** `Remote::query` catches the panic and returns
  `PlayError::Panicked { message }`, so the window lives on. Source: darkroom `AGENTS.md`, "One
  driver, two backends": a query that panics ends that query alone.
- **The OS request to open files reaches the app.** `App::open_files(&mut self, paths:
  &[PathBuf])`, empty by default. The macOS half moves from `darkroom/src/platform/macos/` into
  `palantir/src/host/winit/platform/macos.rs`, beside the code that already configures winit's
  event loop. Linux and Windows pass files in `argv` and need nothing. Refused: unsafe
  Objective-C and a `thread_local!` in the app crate, against "platform differences behind one
  entity". Sources: [Qt `QFileOpenEvent`](https://doc.qt.io/qt-6/qfileopenevent.html),
  [Tauri `RunEvent::Opened`](https://docs.rs/tauri/latest/tauri/enum.RunEvent.html).
- **Window geometry saves and restores as one value.** `WindowGeometry` derives `Serialize` and
  `Deserialize`, keeps the normal bounds while the window is maximized, and
  `WindowConfig::with_geometry` restores it. darkroom stores it in place of `WindowState`.
  Sources: [Qt `saveGeometry`](https://doc.qt.io/qt-6/qwidget.html#saveGeometry),
  [Win32 `WINDOWPLACEMENT`](https://learn.microsoft.com/windows/win32/api/winuser/ns-winuser-windowplacement).

#### Theme

- **`Metrics` holds a type ramp and a resting shadow.** `TypeSteps { caption, body, title }`, 12,
  14 and 16 px by default, and `Metrics::rest_shadow`. `SpaceSteps` gains `xxs`, the 2 px step of
  the densest rows. The recipes read the ramp: `Theme::text` takes `body`, and the tooltip takes
  `caption`. darkroom's `Theme` holds `space: SpaceSteps`, `radius: RadiusSteps`,
  `font: TypeSteps` and the two shadows from one `palantir::Metrics`, so `theme.space.sm` and the
  other reads stay as they are. `StrokeScale::wire` and `wire_at` move to `CanvasTheme`. Refused:
  two copies of the steps and a bridge between them. Source:
  [Fluent 2 type ramp](https://fluent2.microsoft.design/typography).
- **Recipes for the accent button and the flat field.** `ButtonTheme::accent(p, m)`: the accent
  fill and the label in `window_bg`, which every dialog's default action wears.
  `TextEditTheme::flat(&self)`: no padding, margin or chrome, for a field that reads as the text it
  edits. Sources: [GNOME HIG, suggested action](https://developer.gnome.org/hig/patterns/controls/buttons.html),
  [egui `TextEdit::frame`](https://docs.rs/egui/latest/egui/widgets/text_edit/struct.TextEdit.html#method.frame).
- **Frosted glass with a contrast floor is palantir's.** `Chrome::with_frost(self, frost: Frost)`,
  where `Frost { blur, tint_alpha, ink }`. It sets the backdrop's luminance ceiling so that `ink`
  keeps 4.5:1 over any backdrop. The ceiling depends on palantir's dither and its 8-bit storage
  (`darkroom/src/gui/theme/glass.rs`, `before_storing`), so palantir owns the math. Sources:
  [WCAG 1.4.3](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum),
  [Fluent acrylic](https://learn.microsoft.com/windows/apps/design/style/acrylic).
- **Color math is public.** `RgbaF32::luminance(self)`, the WCAG relative luminance of the linear
  channels, and `RgbaF32::contrast_ratio(self, other)`. darkroom keeps `wash` and `toward`: a wash
  composites on encoded values on purpose, and palantir blends in linear light.

#### Input

- **An optional chord argument takes an `Option`.** `MenuItem::shortcut` and
  `MenuItem::shortcut_hint` take `impl Into<Option<Shortcut>>`, as palantir's "Argument shapes"
  asks for an optional argument.

#### Layout and paint

- **An open popup keeps its place.** The first frame that measures the body places it. The place
  changes again only when the anchor moves, the window changes size, or the body stops fitting.
  Then darkroom's canvas menus are `ContextMenu`s and `AnchoredMenu` goes. Source:
  [Win32 `TrackPopupMenu`](https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-trackpopupmenu),
  which places a menu once.
- **Pan and zoom algebra is public, on `TranslateScale`.** `TranslateScale::zoomed_about(self,
  pivot, factor, config: &ZoomConfig) -> Self` and `TranslateScale::scrolled(self, delta:
  ScrollDelta, pivot: Option<Vec2>, config: &ZoomConfig) -> Self`. `ScrollDelta::is_none` and
  `ZoomConfig::clamp` become public. A zoom outside the range steps back toward it and never moves
  against the step, as darkroom's `ZoomRange::zoom_about` holds today. `Scroll` uses the same
  functions. darkroom's `Viewport` converts to and from `TranslateScale` without loss, and
  `ZoomRange` becomes a `ZoomConfig`.
- **A grid can give each row past its list one track.** `Grid::auto_rows(track)`. darkroom's port
  grid drops the `row_tracks` scratch it fills with one track a row. Source:
  [CSS `grid-auto-rows`](https://developer.mozilla.org/docs/Web/CSS/grid-auto-rows).

#### Small additions

- **`Image::as_srgba8` and `Image::into_srgba8`**, the output side of `Image::from_srgba8`.
  darkroom's `view_capture::rgba_image` copies each texel into a new buffer today.
- **Curve math on `CubicBezier`**: `at(t)`, `velocity(t)` and `bbox()`, public. darkroom's `Wire`
  writes the same three. Source: [kurbo `CubicBez`](https://docs.rs/kurbo/latest/kurbo/struct.CubicBez.html).
- **`WidgetId::named(&'static str)`, a `const fn`**, so a fixed id is a `const`. darkroom has 32
  functions whose body is `WidgetId::from_hash("…")`.
- **`RequestedGpu::headless` takes palantir's `DeviceFeatures`**, not wgpu's `Features`. darkroom
  passes `Default::default()` under a lint `expect` today.
- **`OverlayResponse::opened`**, the frame an overlay first shows, beside `dismissed` and
  `close_requested`. darkroom's dialog finds that frame through `ui.state::<T>().is_none()`.

#### Refused

- **A command type** (Qt `QAction`, GPUI actions) to join a menu item, a toolbar chip and a chord.
  One menu bar, one toolbar and about a dozen chords do not pay for a new concept.
- **A stroke alignment setting.** palantir's rule is consistent: an area shape strokes inside, as a
  CSS border; a path strokes on its center, as SVG. Two darkroom sites inflate a rect by hand.
- **A drag that passes from a click-only child to an ancestor that drags.** It changes every button
  in a draggable panel. darkroom lists two drag handles for a node.
- **Tabular numerals in palantir.** darkroom chose the mono face for its figures. Item D3 measures
  them with `Ui::probe_text` in place of a font fact copied by hand.
- **A public checkerboard and a filled-disc shape.** Each serves one darkroom site, and darkroom's
  tiled image already paints a large backdrop as one shape.

## Part 2: Plan

Each batch leaves both workspaces green. A palantir change runs, from `palantir/`,
`cargo drive check`, `cargo drive test` and `cargo drive surface` (commit the new
`.notes/API_SURFACE.md`), and `cargo drive run shot` where a user can see the change. A darkroom
change runs `cargo drive check` and `cargo drive test` from the root. A palantir test goes into
palantir's suite beside the code it tests, with hand-computed values. Whoever addresses an item
deletes it.

### Batch 8: pan and zoom on `TranslateScale`

Files: `palantir/src/primitives/geometry/translate_scale/mod.rs`,
`palantir/src/widgets/scroll/{scroll_state,zoom_config,mod}.rs`; `darkroom/src/gui/widgets/zoom_range/`,
`darkroom/src/gui/pane/graph_ui/gesture/pan_zoom/`, `darkroom/src/gui/pane/image_viewer/camera/`,
`darkroom/src/core/document/viewport.rs`.

- [ ] `translate_scale/mod.rs`: zoom about a point lives only inside `Scroll`
  (`scroll_state.rs:202`). Add `zoomed_about` and `scrolled` as the target design says; make
  `ZoomConfig::clamp` and `ScrollDelta::is_none` public; `ScrollState::apply_zoom` calls them.
  Test: the world point under the pivot stays under it, a zoom outside the range steps toward it,
  and a non-finite factor changes nothing.
- [ ] `zoom_range/mod.rs`: `scrolled`, `fold_scroll`, `zoom_about` and `scroll_to_zoom_factor`
  repeat that algebra. Replace `ZoomRange` with two `ZoomConfig` constants and call the palantir
  functions through `Viewport`'s `From` conversions. Move the cases of `zoom_range/tests.rs` that
  test the algebra to palantir; keep the darkroom cases that test the canvas's and viewer's
  ranges. Hand-compute the new wheel step so a notch zooms by the same factor as
  `1.0025^line_px` does today.

### Batch 9: an open popup keeps its place

Files: `palantir/src/widgets/popup/mod.rs`, `palantir/src/primitives/layout/anchor/`,
`palantir/src/layout/`; `darkroom/src/gui/pane/graph_ui/gesture/{anchored_menu,node_menu_ui,new_node_ui,tie_ui}/`.

- [ ] `popup/mod.rs`: the place is computed again in each frame, so an open popup moves when its
  body changes size. Keep the first measured place while the anchor, the window size and the fit
  hold. Test in `popup/tests/placement.rs`: a body that shrinks after it opened near the bottom
  edge stays where it opened.
- [ ] `anchored_menu.rs`: the latch by hand, plus the context menu's look copied from the theme.
  Open the canvas menus with `ContextMenu::open(ui, id, point)` and `ContextMenu::for_id(id)`,
  `max_size` where `PopupShape::max_height` was. Delete `AnchoredMenu` and `PopupShape`.

### Batch 10: window geometry as one value

Files: `palantir/src/window/window_geometry.rs`, `palantir/src/window/` (`WindowConfig`),
`palantir/src/host/winit/window/`; `darkroom/src/core/io/preferences/mod.rs`,
`darkroom/src/gui/app/mod.rs`, `darkroom/src/lib.rs`.

- [ ] `window_geometry.rs`: the geometry cannot be saved, and while maximized it gives the
  maximized size. Derive `Serialize` and `Deserialize`, keep the normal bounds, and add
  `WindowConfig::with_geometry`. Test: maximize, then read the normal size the window had.
- [ ] `preferences/mod.rs` `WindowState` and `app/mod.rs:246` `track_window_state`: a copy of the
  type, and a rule by hand that keeps the last normal size. Store `WindowGeometry`, and restore it
  in `lib.rs` with `with_geometry`.

### Batch 11: small API additions

Files: `palantir/src/widgets/context_menu/menu_item.rs`, `palantir/src/primitives/paint/`
(`Image`), `palantir/src/primitives/geometry/cubic_bezier/mod.rs`,
`palantir/src/primitives/identity/widget_id/mod.rs`, `palantir/src/gpu/device/requested_gpu.rs`,
`palantir/src/widget_core/overlay_response.rs`, `palantir/src/layout/` (`Grid`); and the darkroom
callers.

- [ ] `menu_item.rs:55,61`: `shortcut` and `shortcut_hint` take a `Shortcut`. Take
  `impl Into<Option<Shortcut>>`; `menu_bar.rs` `item` drops its `match`.
- [ ] `Image`: no byte view on the output side. Add `as_srgba8` and `into_srgba8`;
  `view_capture.rs:74` `rgba_image` and `shot.rs` stop the texel copy.
- [ ] `cubic_bezier/mod.rs:37`: `bbox` is crate-private and there is no `at` or `velocity`. Make
  the three public; `frame/wire/mod.rs` `Wire` holds a `CubicBezier` and drops its copies.
- [ ] `widget_id/mod.rs:48`: no `const` constructor. Add `WidgetId::named`; turn darkroom's 32
  literal-id functions into `const` items.
- [ ] `requested_gpu.rs`: `headless` takes wgpu's `Features`. Take `DeviceFeatures`;
  `driver/offscreen.rs` drops its lint `expect`.
- [ ] `overlay_response.rs`: no edge for the first frame. Add `opened`; `discard_dialog.rs` drops
  the `ui.state::<DontAskAgain>(…).is_none()` probe.
- [ ] `Grid`: add `auto_rows(track)`; `node_ui/mod.rs` drops `row_tracks` and `port_row/mod.rs`
  its refill.

### Batch 12: darkroom-only fixes

Files: as each item names.

- [ ] D1, `image_viewer/mod.rs:388` `header`: the header's width comes from last frame's controls
  width, so for one frame after a resize the two overlap. Put the header and the controls column
  in one `Panel::hstack` row, the header hugging with `TextWrap::Ellipsis`, and drop the
  `response_for` read.
- [ ] D2, existing sugar: 12 `Align::new(HAlign::…, VAlign::…)` equal a constant
  (`Align::TOP_LEFT`, `LEFT`, `TOP`, `TOP_RIGHT`, `RIGHT`); 7 square
  `(Sizing::fixed(s), Sizing::fixed(s))` are `.size(s)`; 11 `(Sizing::FILL, Sizing::FILL)` are
  `.size(Sizing::FILL)`; 18 `.left.clicked()` are `.clicked()`; the empty
  `Panel … .show(ui, |_| {})` calls (`support::dot_cell`, `support::hspacer`, the palette's focus
  bar, `node_footer.rs` `bar_segment`) are `Block`s.
- [ ] D4, tests: `menu_bar.rs:208`, `node_footer.rs:193` and `tie_ui/tests.rs:364` each list
  widgets through `ui().inspect` with a helper of their own. Use `Harness::widgets(filter)` or
  `Harness::find` where they serve, and one shared helper where they do not.
  `node_footer.rs:189` rounds to the 1/64 px grid that `line_height_for` already returns; drop the
  rounding.

### Batch 13: fold the decisions and delete this file

Files: `palantir/AGENTS.md`, `darkroom/AGENTS.md`, `.notes/review-palantir-api.md`.

- [ ] Each target-design bullet that landed becomes a decision bullet in the `AGENTS.md` of the
  system it governs: widgets, hosts, theme, input and layout in `palantir/AGENTS.md`; the driver's
  use of `is_busy`, the waker and `Remote::settle` in `darkroom/AGENTS.md`, "The editor harness".
  Code that cites one cites it by document and title. Then delete this file.

## Decisions

- **A busy app is a trait method, read after a frame.** `App::is_busy` reads the app's own state
  when a settle asks. Refused: a flag the record sets on `Ui`, stale when no frame records, the
  case a settle waits in. Sources: Playwright's auto-wait
  (https://playwright.dev/docs/actionability), which waits on the page's own state.
- **A waker is a type-erased closure.** `Waker` holds an `Arc<dyn Fn() + Send + Sync>`, as the
  host is chosen at run time and the winit handle is generic over the app. Refused: an enum of the
  hosts, which puts `HostHandle<T>` and its `T` into `Ui`. Source: egui's `Context`, which any
  thread calls (https://docs.rs/egui/latest/egui/struct.Context.html#method.request_repaint).

- **One `Selection` for every two-pack theme.** palantir's `TabSelection` became `Selection`, and
  `ButtonTheme`'s pick takes it as `TabsTheme`'s does, so "selected" keeps one meaning across
  widgets. Refused: a `bool` pick, against palantir's rule that a flag that selects behavior is an
  enum; a second enum for buttons, two names for one idea.
- **`show_with` adds shapes to a leaf.** `Button` stays a leaf node, as its label is measured as a
  leaf's text; the body adds shapes in the button's space. Refused: a container button, whose
  label would stop measuring as a leaf's does.

## Changed

- darkroom's steps are its own constants (`TEXT`, `SPACE`, `RADIUS`, `STROKE` in
  `gui/theme/mod.rs`) on palantir's step types, which build the `Metrics` it hands palantir, so a
  retune of `Metrics::DEFAULT` moves nothing in darkroom. `ElevationScale` stays darkroom's (see
  Declined), and hands palantir the floating shadow.
- D3 landed with Batch 7: `support::figure_width` measures the figures with `Ui::probe_text`, as
  the type scale it lived on went.

- A view capture asks frames (`Ui::request_repaint`) until its image lands, in place of reading
  as busy: its image comes in a frame, not with a wake, so `App::is_busy` is the previews alone.
- `App::quit` sets a flag that closes the window as the pass ends (`Ui::close_window`), as the
  quit runs in the record without the `Ui`; the offscreen driver reads the close in
  `Harness::window_commands`.
- `Driver::wait_run` keeps its check for a wake that came after the settle, through
  `Harness::wait_for_wake` with a deadline that has passed.

- `TextEdit::buffered`: the draft takes the current value on every frame that follows one the
  field was not focused in, not only while idle, so a session that opens with a `set_focus`, as
  `EditableLabel`'s does, starts from the value.
- `EditableLabel`: its response reports `editing` beside `committed_text`, so a caller reads a
  click as one on the label alone; the first Escape drops the selection the editor opens with,
  as in every `TextEdit`, and the second the edit.

- `drag_value/mod.rs`: keeps its own draft. It parses the draft on each keystroke and shows the
  value live, where `TextEdit::buffered` hands the draft over only on its commit; moving it
  would change what a scrub field shows while typed in.
- `const_editor/mod.rs` `Field::empty`: a `Panel` in the editor's resting look around a `Text`,
  in place of a read-only `TextEdit` (see Declined). It shows the mark and takes no click, as its
  doc said it did.
- `const_editor/mod.rs` `any_edit`: drops its check of the shown text, as the buffered field
  never hands that text over.

- `node_palette.rs` `Row::show`: stays a `Panel`. A row holds child widgets (a focus bar, a
  name, a description), and a `Button` is a leaf that lays out no children.

- `session/mod.rs` `menu_shortcut`: a `MENU_CHORDS` table and one `find` in darkroom, with no
  palantir API, as the search may stop at the first chord that fired (see Declined).

## Declined

- Batch 7, `Metrics::rest_shadow`: no palantir recipe casts a resting shadow, so the step would
  be a field palantir never reads; darkroom's `ElevationScale` keeps it.
- Batch 7, `ButtonTheme::accent`: palantir's palette has no tier for an accent under the pointer,
  and darkroom's lifts toward white as a port does, a rule of its own; `primary_button` stays in
  darkroom.

- Batch 6, `App::open_files` and the move of darkroom's macOS hook into palantir: palantir pins
  `objc2-app-kit` 0.2 for winit 0.30, and the hook is written for `objc2` 0.6. The move rewrites
  unsafe Objective-C against the older API, with two new dependencies, and no macOS build here to
  check it; it waits for palantir's move to winit's next release.

- Batch 4, `TextEdit::read_only`: one darkroom field wants it, and it shows only a mark that no
  one selects or copies. Read-only touches typing, the edit menu, undo and the input method in
  palantir; a non-interactive box serves the field.

- Batch 2, `Response::tooltip`: `Response` lives in `widget_core`, below `widgets`, where
  `Tooltip` lives. The method either breaks palantir's layering or puts an `impl Response` outside
  its file, and darkroom's `tooltip_after` already makes the pair one call. `Response` keeps its
  `&Ui`.

- `Ui::pressed_shortcut`: the gap it closed does not exist. A frame that receives a press records
  again with no input, and that pass reads every chord a short-circuit skipped, so each stays
  watched. palantir now documents this on `Ui::shortcut_pressed` and pins it in
  `a_shortcut_read_skipped_behind_one_that_fired_stays_watched`.
- `shortcuts.rs:36` `emit`, Backspace behind Delete: not a bug, for the same reason. The issue
  entry is deleted, and the doc comments that described the trap are corrected.
