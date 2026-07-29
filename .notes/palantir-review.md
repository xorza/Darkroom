# `palantir/src` — review findings

Scope: every module under `palantir/src/` (production code only; test files,
benchmark implementations, and the APIs tests reach for were not reviewed).

Every item below was re-verified against the working tree at `6a4996e1` —
anchors re-pinned, claims re-read, the `caches` bench re-run. Five findings did
not survive and are recorded under **Checked and dropped**; six duplicated
benchmark items were folded into **Benchmark gaps**.

**When you address an item, delete it from this file.** These are observations,
not designs — each says what is wrong, not how to fix it. Performance items
state whether the claim is **measured**, **derived** from a measurement, or
**unproven**; nothing below should be credited with a speedup until its named
benchmark moves.

Sections are grouped by the code they touch and ordered by the impact of their
heaviest item; items within a section are ordered the same way.

---

## `ui/` — frame lifecycle

The only place a whole-pass skip is still on the table, and the one blocker
that keeps two tree walks apart.

- [ ] **The frame-skip gate is computed after the work it could skip.**
      `cascade_fingerprint` takes only `(&Forest, Display)` and already covers
      root identity, complete subtree authoring, placement, surface size and
      scale (`scene/cascade/mod.rs:633-675`) — but `Ui::post_record`
      (`ui/mod.rs:391`) runs `layout_engine.run` at `:396-401` and only
      computes the fingerprint at `:412`. On an identical recorded frame the
      retained `Layout` is discarded and rebuilt before the marker that proves
      it didn't need to be. The cached layout pass it would skip measures
      2.46 µs measure + 0.83 µs arrange (see `layout/`, below). **Derived.**

      Two things make this more than moving one line, and either one silently
      corrupts a frame if missed:

      - `frame_runtime.prev_cascade_fp` (`ui/frame.rs:119`) tracks the most
        recent record pass, *including an earlier pass in the same frame*. The
        scene the damage snapshot belongs to is a different marker — pass B can
        equal pass A while both differ from the last rendered frame — so one
        retained field cannot serve both roles.
      - `TextSystem::end_frame` (`text/system.rs:71-76`) retains only rows
        whose hot bit was set *during measure*. Skipping layout marks nothing
        hot, so every reuse row is dropped and the next real layout pays a full
        re-measure. Nothing couples the two — `end_frame` is called
        unconditionally at `ui/mod.rs:435` — so a skip added without also
        skipping it reads as a regression two frames later, for the wrong
        reason.

- [ ] **Same-frame record replay is a second lifecycle protocol, and nothing
      uses it.** A double-layout frame runs record, rollups, cascade and layout
      twice (`ui/mod.rs:236-273`: warmup, pass A at `:256`, pass B at `:271`).
      It is why `frame_had_action` (`input/mod.rs:320`) exists with its own
      reset semantics, and it is the direct blocker on fusing the cascade and
      damage walks (see `scene/`, below). **`request_relayout`
      (`ui/mod.rs:570`) has zero callers in `src/`** — not one widget this
      crate ships, which its own doc at `:556-569` says. The retry path is
      carried entirely for `frame_had_action`.

## `layout/` — the measure-cache restore is now the hot path

A direct consequence of shipping the arrange replay. Re-measured on `caches`
(min µs, 100 samples): `measure/cached` = **2.46 µs measure, 0.83 µs arrange**;
`heavy/measure/cached` = 1.21 / 0.42; `broad/measure/cached` = 0.38 / 0.25. On
a root cache hit measure does no measuring, so that 2.46 µs is almost entirely
`restore_after_cache_hit` (`layout/engine.rs:228`). Arrange — the half everyone
was looking at — is now the cheaper one, by ~3× on the arm that dominates.

The absolute stake is small: ~3.3 µs for the whole cached layout pass. Weigh
any fix here against the `ui/` skip above, which subsumes all of it.

- [ ] **`scroll_content` is a dense `Vec<Size>` sized to every node** —
      cleared and zero-filled per layer per frame (`layout/mod.rs:84-85`),
      duplicated in the snapshot (`layout/cache/mod.rs:96`), and slice-copied
      on every cache hit (`layout/engine.rs:236`) — for data with one producer
      (`layout/scroll/mod.rs:44`) and two consumers: `layout/scrollbars/mod.rs:199`
      within the same pass, and `widgets/scroll/mod.rs:199` across frames. Only
      the second needs the cache round-trip. **Derived.**

- [ ] **The `text_spans` rebase is a per-node loop over the whole tree on
      every cached frame** (`layout/engine.rs:242-251`). On a root hit
      `dest_start` and `cached.text_shapes_base` are both 0, so
      `dest_start + snap_span.start - cached.text_shapes_base` is the identity
      and the loop writes each span back unchanged — where a `copy_from_slice`
      would do, if the snapshot normalized empty spans at capture instead of
      here. **Derived.**

- [ ] **The `cache_rebuild` arm adds a second per-node nested loop over
      `SLOT_COUNT`** (`layout/engine.rs:252-263`), NaN-testing each slot, plus
      another whole-subtree `copy_from_slice` for `available_q`.

- [ ] **`grid.hugs` is restored on grid-bearing hits but read by neither
      replay branch.** `replay_arranged` (`layout/engine.rs:952-991`) reads
      only `cache.previous.nodes.rect` and `arrange_src`, so `restore_subtree`
      (`:268-272`) is dead work on the hot path and live only on the
      resize-bail path. The "three coordinated edits" contract documented at
      `layout/engine.rs:70-76` now costs more than it protects. Note the
      restore is already gated on `tree.subtree_has_grid` (`:268`) — a
      grid-free subtree pays one bit test, not a copy — so this is a
      grid-fixture cost, not a universal one.

- [ ] **`MeasureCache::capture_tree` runs six release `assert_eq!`s per tree
      per rebuild frame** (`layout/cache/mod.rs:213-218`) plus a seventh inside
      the text branch (`:245`). All seven are internal column-length
      invariants, not public-API contracts — the category the crate's own
      assert policy reserves `debug_assert!` for.

- [ ] **Layout driver identity is restated across four exhaustive
      `LayoutMode` matches** — `measure_dispatch` (`layout/engine.rs:813`),
      `arrange` (`:876`), `intrinsic::content_intrinsic`
      (`layout/intrinsic/mod.rs:196`), and `arrange_depends_only_on_slot`
      (`layout/types/layout_mode.rs:38`). Adding a driver needs three edits and
      `Scroll` delegates differently in each phase; the fourth carries the
      arrange-replay soundness contract with no compile-time tie to the other
      three.

## `scene/cascade/` — the preflight verifies O(N), then the walk discards it

- [ ] **`CascadesEngine::can_update` (`scene/cascade/mod.rs:527`) does a full
      `Rect` slice comparison (16 B/node) plus a full `subtree_ends` zip per
      layer** (`:552`, `:555-562`), on every frame where anything changed at
      all. **Derived.**

- [ ] **A paint-row *count* change is only detected mid-walk.**
      `run_tree::<true>` bails at `old_span.len != new_span.len`
      (`scene/cascade/mod.rs:822`), and `run` then calls `run_full` and redoes
      every layer from scratch (`:518-521`) — after the preflight scans and
      after however much of the incremental walk already ran. Adding one shape
      to one node pays preflight + partial incremental + full rebuild.
      `OpenFrame::paint_rows` (`scene/tree/recording.rs:34`) already maintains
      that count during recording, for `PaintAnimEntry::row`; the preflight
      never consults it. **Derived.**

- [ ] **`run_tree` carries a recoverable-failure return type for that one
      late bail alone** (`scene/cascade/mod.rs:695`, bail `:822`, success
      `:922`), and `run_full` immediately `assert!`s the same value can never
      be false (`:598`).

## `scene/damage/` — the moved-subtree leg, and one 423-line body

- [ ] **Tier 1.5 does a `prev_map` hash probe, a `union_screens` fold, a
      `copy_from_slice`, and a `cascade_input` write for every *painting* node
      in a jumped subtree** (`scene/damage/mod.rs:580-658`, probe at `:601`;
      non-painting nodes skip at `:599`) — every frame of every scroll gesture
      over a long list, where the structure inside the jump is known identical
      to last frame. The retained snapshot is keyed `WidgetId -> NodeSnapshot`
      (`:101`), so there is no stable slot the descendants could be reached
      through sequentially; identity, additions, removals and reparenting all
      share the one map. **Unproven**; the code notes the leg was already
      optimized once (a per-row hash matcher that was ~18% of a scrolling
      frame).

- [ ] **`DamageEngine::compute` is 423 lines** (`scene/damage/mod.rs:291`),
      holding five diff tiers, the `MOVED_SUBTREE` sentinel round-trip
      (`:455` → `:572`), a nested mini parent-stack (`:579-597`), the
      predamage fold, and the eviction tail in one body.

## `scene/` ↔ `ui/` — two walks over the same tree, back to back

- [ ] **Cascade and damage build and copy the same rows twice on the
      incremental path.** `run_tree::<true>` builds paint rows into
      `paint_scratch` then copies into `paint_arena.rows`
      (`scene/cascade/mod.rs:825-826`) — the full path writes `paint_arena`
      directly (`:829`) and pays no copy. Damage then reads `paint_arena.rows`
      and copies into `arena.snaps` (`scene/damage/mod.rs:609`, `:398`). A
      dirty node's rows are built once and copied twice, with two ancestor
      stacks maintained over the same ancestry (cascade's `Frame` stack vs
      damage's `parent_stack`).

      The two cannot be fused while record replay exists: cascade runs inside
      `record_pass` (`ui/mod.rs:379`, twice on a double-layout frame) while
      damage runs once at `ui/mod.rs:298-305` because it needs `ids.removed`
      from `finalize_frame`. Fusing would make damage run twice and the first
      pass's diff would corrupt the snapshot baseline. See the `request_relayout`
      item under `ui/`.

## `input/` — the fast path is gated on the rare case

- [ ] **`frame_quiescent` requires `pointer_pos.is_none()`**
      (`input/mod.rs:926-935`), so the whole-interaction-half short-circuit
      (`:966-975`) fires only when the cursor has left the window. Whenever the
      pointer is anywhere over the surface, every widget pays the three-button
      capture scan, the drag math, the `pointer_local` transform, and the
      scroll lookup, whether or not anything is captured. **Unproven** — no
      bench distinguishes pointer-present from pointer-absent idle frames.

- [ ] **`InputState::on_input` is 253 lines** (`input/mod.rs:571`) — one match
      whose arms each derive their own `observable` and `frame_had_action`
      answer from different rules; the rules are documented on the field
      (`:296-319`) rather than at the arms that implement them.

## `renderer/frontend/` — two long bodies, one shared prologue

- [ ] **`emit_one_shape` is 255 lines**
      (`renderer/frontend/encoder/mod.rs:294`) with per-variant payload
      assembly inline in the dispatch.

- [ ] **`ComposeSession::curve` and `::arc` share a prologue and an epilogue
      verbatim** (`renderer/frontend/composer/mod.rs:1131` and `:1209`): the
      `scale` / `display` / `width_phys` / `cap` opening, the nine-line
      `stroke_bbox_urect` call with identical arguments, the
      `enter_higher_kind(PaintTier::Curve, …)` guard, the `ColorU8`
      conversion, and a `push_sub_instances` whose `CurveInstance` differs only
      in the `p0..p3` lanes and `kind` — ~38 of 77 and 67 lines. The middles
      are genuinely different math (control-point rotation vs centre + angle
      shift; control-polygon length vs `r·|sweep|`) and both are commented as
      such, so only the two ends are worth extracting.

## `renderer/backend/` — one long body, two near-identical passes

- [ ] **`WgpuBackend::submit` is 265 lines** (`renderer/backend/mod.rs:394`)
      mixing the upload phase, three debug-overlay paths, pass selection, the
      backbuffer copy, timestamp resolve, and belt recall.

- [ ] **`run_dim_pass` and `run_overlay_pass` are the same shape**
      (`renderer/backend/mod.rs:666`, 15 lines, and `:984`, 22 lines):
      `begin_load_pass` plus one `self.debug.draw_*` against
      `fmt.quad.select(false)` and `self.gradient.bg`. Beyond the target view
      and the draw call, the overlay arm also creates its own view from a
      texture and threads a `count`. Small — a shared helper saves ~10 lines.

## `widgets/scroll/` — one long body with a repeated inner loop

- [ ] **`Scroll::show` is 280 lines** (`widgets/scroll/mod.rs:464`) covering
      wheel/pinch routing, zoom gating, two bar gestures, wrapper patching, and
      the nested record — with the state mutation buried in a 90-line block
      expression (`:548-632`).

- [ ] **Its two per-axis loops are near-identical**
      (`widgets/scroll/mod.rs:566-594` and `:595-622`): both iterate
      `[(Axis::Y, …), (Axis::X, …)]`, both `continue` on `!panned`, both call
      `scrollbars::bar_geometry` with the same five arguments derived the same
      way. The second adds two guards (`clicked()`, `pointer_local`) and maps
      to `TrackPage` instead of a ratio pair, so a merge saves the argument
      derivation and little else.

## `renderer/gradient_atlas/` — repeated search over monotonic input

- [ ] **`bake_stops` restarts the stop search at index 1 for every one of the
      256 LUT texels.** `lerp_at` (`renderer/gradient_atlas/bake.rs:47`) opens
      with `let mut upper = 1;` at `:53` and walks forward; the stops are
      sorted and the caller's sampled `t` increases monotonically (`:35-44`),
      so the cursor can only move forward. `O(LUT_SIZE × stop_count)` for what
      the data supports as `O(LUT_SIZE + stop_count)`. Bounded: `MAX_STOPS = 8`
      (`primitives/brush/gradient/stops/mod.rs:8`), and `bake_stops` runs only
      on an atlas row miss (`gradient_atlas/mod.rs:339`), not per frame — so
      the worst case is ~2 k float compares on a cold path. Cheap to fix,
      cheap to skip.

## Benchmark gaps

Each gates a finding above; none is a finding on its own.

- [ ] **Identical-record lifecycle** — record + rollup + gate + damage with no
      forced frontend work. `frame/cached_cpu` is the wrong instrument: it
      deliberately substitutes a `Full` plan after `Damage::Skip`
      (`ui/bench.rs:265-274`) so every CPU arm measures the same pipeline, and
      therefore always includes whole-tree encode + compose. Control:
      `frame/cached_cpu`.
- [ ] **Cascade with a paint-row *count* change.** Control: a paint-only change
      (where the incremental walk succeeds).
- [ ] **Scroll over a long list** — moved subtree, no authoring change; probes
      vs bytes copied. Control: a static list.
- [ ] **`restore_after_cache_hit` split by column**, so which of its four
      costs dominates stops being a guess. Control: forced miss.
- [ ] **Maximum-stop LUT bake.** The `gradient` bench has one arm
      (`gradient/repeated_chrome`, `renderer/frontend/bench.rs:89`) and never
      bakes a LUT. Control: a two-stop gradient.
- [ ] **Idle frame with the pointer over the surface vs off it**, for
      `response_for`.

---

## Checked and dropped

Carried by an earlier revision, re-read at `6a4996e1`, and removed. Recorded so
they are not re-derived.

- *`scroll_wrappers`' destructure "does not cover the whole node".* It does.
  `clip: _` and `transform: _` (`widgets/scroll/mod.rs:313-314`) are **named**
  bindings, not elided, carrying a comment that says why they are re-derived in
  `Scroll::show`. Adding a `Node` field still breaks this function, which is the
  entire guarantee the destructure exists for.
- *`scroll_delta_for` is a linear scan per widget per frame.* Self-defeating:
  the scan is over `frame_target_deltas` (`input/mod.rs:502-507`), which the
  finding itself notes is "almost always empty or one entry" — i.e. already
  O(1). There is nothing to recover.
- *`FrameProcessing::SingleLayout` / `DoubleLayout` are misnamed.* They are
  accurate today — each record pass runs its own `post_record` + layout, so the
  record-pass count and the layout count are the same number. The objection was
  anticipatory, conditional on the `ui/` skip landing; re-raise it then.
- *The retained damage snapshot's `WidgetId` keying is a finding.* It is the
  explanation for why the moved-subtree leg has no sequential slot to walk, not
  an independent defect. Folded into that item.
- *The composer's kind-order recovery machinery.* Verified intact and correctly
  anchored — fixed order quads → text → meshes → images → curves
  (`renderer/frontend/composer/mod.rs:54-67`), with `HigherKindRects` (`:82`),
  two `TextRectGrid`s (`:127-128`), `quad_forces_flush` (`:378`), `closed_hit`
  (`:405`), and the strict-bounds batch rule (`:1497-1507`) all existing to
  detect and flush the cases where that reorder would change the picture. But
  the item's own conclusion is that nothing local there is worth touching while
  the order-preservation question is open, which makes it a do-not-touch note.
  Moved below.

## Do not re-attempt

Not findings. Recorded so the ground is not re-walked; each was examined and
closed.

**Order-preserving GPU replay.** The composer's largest subsystem exists to undo
the backend's fixed per-kind reorder (anchors above). Everything in it is a
reasonable optimization *of* the current design, so nothing local is worth
touching while the order-preservation question itself is open. Reopening means
reopening the replay order, not the flush rules.

**Shipped since the source audits.** Layer-ordered keyboard capture (a `Modal`
sees Escape under a capturing `Popup`; a `TextEdit` inside a popup receives
typing); `HostHandle::run_on_main` returning `Result<(), HostDisconnected>`;
`DragValue`'s `inherit_chip_node` carrying node policy across the chip→editor
swap; the **arrange replay** (arrange 89.18 → 1.17 µs on `measure/cached`,
30–150× across arms, whole layout pass 92.4 → 4.36 µs); `Sense::ABSORB_POINTER`
replacing `Modal`'s `BLOCK` and `Popup`'s hand-written eater sense; the
image-shader nearest-filter branch; scissor deduplication through one
`cur_scissor` state; adjacent same-texture image-draw coalescing (`images/shared`
3.6–4.1 → 1.3 µs); one-entry text-grid spill; `encode_node`'s dropped
`ChromeRow` copy; `classify_frame` → `take_frame_plan`; the paint-sink pipeline;
`text/mod.rs` split 1032 → 401 production lines.

**Withdrawn on measurement.**
- *Encoded text-cache sweep cadence* — the whole pass is ~1.6 ns per live row
  (`encoded_cache_sweep` bench); a cadence gate would trade uniform per-frame
  cost for a spike with no average to recover.
- *Backend bind-state tracking split, including text* — ~11 ns per recorded
  step, ~29 ns per text batch; the full fix recovers ~4 µs on a fixture
  engineered to produce 256 consecutive text batches, which real frames don't.
- *Backend last-binding cache* — structurally impossible alongside run
  coalescing: once adjacent runs merge, no two consecutive lookups can share an
  id, so a one-entry cache has a guaranteed 0% hit rate.

**Rejected after being built.**
- *A `Configure` delegation macro for the 20 identical `node_mut` impls.*
  `node` is private to each widget module, so a single roster in
  `widgets/mod.rs` needs 19 visibility escalations; per-widget invocation gives
  up the roster, which was the only benefit.
- *Live keyboard-ownership resolution.* With popups B then A recording in that
  order, B reads while topmost-so-far and A reads after displacing it, so
  *both* receive the key. Stable-during-frame / resolve-at-end is right; only
  the starting value can be stale.
- *An overlay recorder unifying `Popup` / `Modal` / `Tooltip` chrome and
  placement.* The three resolve different theme slots against different fields,
  placement is already two library calls, and the two scrims are structurally
  different — the shared type's fields would be the differences.

**Examined and correctly rejected in the source documents.** Physical module
reorganization; owner-partitioned retained render targets; range-aware mesh
uploads; a retained command buffer or compose cache; merging typed
render-buffer columns; unifying the image/curve/mesh/text pipelines; globally
sorting higher-kind primitives; replacing retained scratch with locally
collected iterators; `Backbuffer::size`'s cached field; two hand-built
full-viewport quads; the debug dim quad's missing `FillKind::SOLID.with_fast()`;
`shader_template::specialize`'s startup string copies; merging the three "did it
change?" hash families; `Cascades::by_id.clone_from`; the composer's
`OcclusionPruner`; per-layer iteration over five layers; `Tree::compute_rollups`
re-hashing.
