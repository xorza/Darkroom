# GUI Module — Research Notes

Research against industry standards (Blender, Unreal Blueprints, Houdini, Substance Designer, Nuke) and best practices for node graph editors.

**Related code review:** See `REVIEW.md` for code-quality findings (dead code, duplication, consistency issues).

---

## Critical Missing Features

### 1. Multi-Select (CRITICAL)

**Current:** Single node selection only (`Option<NodeId>`). No box select, no shift-click, no ctrl-click.

**Industry standard:** Universal across all major editors:
- LMB drag on background = box/marquee select (Blender, Unreal, Houdini)
- Shift+click = add to selection
- Ctrl+click = toggle selection
- Shift+drag = additive box select
- Ctrl+A = select all

**Impact:** Highest-priority gap. Every user from any other node editor expects this. Multi-select is prerequisite for bulk delete, duplicate, align, comment boxes, and grouping.

**Conflict:** Current LMB-drag-on-background starts the connection breaker. Industry convention uses LMB-drag for box select and reserves breaker/cutter for a modifier (e.g., Ctrl+drag or dedicated tool).

**Changes needed:** `selected_node_id: Option<NodeId>` → `selected_node_ids: HashSet<NodeId>`, add `BoxSelect` interaction mode, update all operations (move, delete) to work on selection set.

### 2. Search in New Node Popup (HIGH)

**Current:** Categories with expandable sections, no search/filter, no keyboard navigation.

**Industry standard:** Every editor (Blender Shift+A, Unreal right-click, Houdini Tab) has an auto-focused search field that filters as you type. Arrow keys + Enter for keyboard navigation. Results in single-column flat list.

**Recommended search ranking:** Exact prefix > word-boundary match > substring > subsequence. Match against both name and category. Case-insensitive.

**Additional features used by professional editors:**
- Context-aware filtering when dragging from a typed port (Blender, Unreal show only compatible nodes)
- Recently used section (3-5 items, Houdini Tab menu)
- Favorites (Substance Designer)

### 3. Keyboard Shortcuts (HIGH)

**Current:** Only Escape and right-click for cancel. No other keyboard shortcuts.

**Industry-standard shortcuts missing:**

| Action | Standard | Notes |
|--------|----------|-------|
| Delete node(s) | Delete | Universal |
| Undo / Redo | Ctrl+Z / Ctrl+Y | Universal |
| Copy / Cut / Paste | Ctrl+C/X/V | Universal, internal clipboard |
| Duplicate | Ctrl+D | Blender Shift+D, Unreal Ctrl+D |
| Frame all | Home or A | Houdini H, current "a" button has no shortcut |
| Frame selected | F | Houdini G, current "s" button has no shortcut |
| Search nodes | Ctrl+F | Houdini /, Unreal Ctrl+F |
| Add node menu | Tab or Shift+A | Houdini Tab, Blender Shift+A |
| Mute/bypass | M or Q | Houdini Q, Blender M |
| Add comment | C | Unreal C |

### 4. Connection Type Checking (HIGH)

**Current:** Only cycle detection and port-kind compatibility (Input↔Output, Trigger↔Event). No data type validation.

**Industry standard:** Blender and Unreal prevent connecting incompatible data types (e.g., Float→String). Compatible ports highlight during drag; incompatible ports dim. Some editors auto-insert conversion nodes.

**Recommended additions:**
1. Type check on connection attempt (reject or warn on mismatch)
2. Visual highlighting of compatible ports during drag
3. Dimming of incompatible ports during drag

---

## Important Missing Features

### 5. Node Collapse/Expand

Every major editor supports collapsing nodes to just header+ports (Blender H key, ComfyUI toggle button, Houdini). Essential for managing complex graphs. Not currently implemented.

### 6. Comment Boxes / Frames

Unreal (C key), Blender (Ctrl+J Frame), Houdini (Shift+O network box, Shift+P sticky note). Colored rectangles with titles that contain and move with nodes. High organizational value, moderate implementation effort.

### 7. Reroute / Dot Nodes

Minimal pass-through nodes that redirect connection wires for cleaner routing. Common in Blender, Substance Designer, Unreal. Low implementation cost.

### 8. Node-Type Color Coding

Professional editors universally color-code node headers by category (math=blue, image=green, etc.). The current implementation has no category-based coloring. This is the most impactful visual hierarchy improvement.

### 9. Find-in-Graph (Ctrl+F)

Search for existing nodes by name/type in the current graph. Houdini (Ctrl+F, /, Ctrl+G for next match), Unreal (Ctrl+F, Ctrl+Shift+F cross-blueprint). Results navigate view to found node and select it.

---

## Zoom & Pan

### Current Implementation: Correct

- **Zoom formula** `exp(delta * 0.08)` is industry-standard exponential/multiplicative zoom. Each step is ~8.3% change — within the 5-15% sweet spot. Matches D3.js, egui conventions.
- **Zoom range** [0.2, 4.0] is appropriate. Blender uses similar ranges.
- **Zoom centering on pointer** is correct and universal.
- **Scroll handling** correctly distinguishes Point (touchpad) vs Line/Page (mouse wheel).
- **Pan: no inertia needed** — professional editors (Blender, Unreal, Houdini) don't use pan inertia.

### Minor Improvements

- **Animate fit-all / view-selected** transitions (150-250ms ease-out) — Blender and Houdini animate these. Currently instant.
- **view_selected resets zoom to 1.0** — should preserve current zoom and only center the node.
- **fit_all padding** uses fixed 24px — consider proportional (5-10% of viewport). ReactFlow uses 10% by default. 24px is only ~1.25% of a 1920px display.
- **Ctrl+scroll suppression** (line 717): When Command is held, `zoom_delta` is forced to 1.0. This prevents conflict with macOS accessibility zoom. Defensible but should be documented.

---

## Connection Rendering

### Current Implementation: Correct and Standard

- **Horizontal cubic Bezier** with control offset `(dx*0.5).max(35*scale).min(90*scale)` matches all major editors.
- **3-quad polyline mesh + feather** is the standard anti-aliased curve technique (same as Dear ImGui, egui internal, per "Drawing Lines is Hard" article).
- **Per-segment hit testing** with squared-distance is industry-standard for tessellated curves.
- **Simple direct Bezier routing** (no obstacle avoidance) matches Blender, Unreal, Houdini, Substance, Nuke — none do automatic edge routing.
- **Orientation-based segment intersection** for breaker tool is correct. The `1e-6` epsilon is unnecessarily tight for f32 screen coords but harmless in practice.

### Potential Improvements

- **Fixed 35 sample points** per curve is over-sampled. Adaptive count based on curve length would reduce vertex count: `max(8, min(35, arc_length / 15))`. Not a problem until 100+ connections.
- **Bounding rect pre-check in hit testing** — the bounding rect is computed but not used as early-out in `hit_test()`. Adding `if !bounding_rect.expand(width).contains(pos) { return false; }` would skip most curves cheaply.
- **Animated flow direction** (moving dashes along wire) — Unreal uses this for execution flow. Most impactful visual feedback addition for communicating data flow direction.
- **`connection_feather` is intentionally not scaled** (rendering parameter, not visual). Should be documented to prevent "bug fix" attempts.

---

## Node Rendering & Layout

### Current Implementation: Sound

- **Fixed header + port rows** layout is standard across all editors.
- **Inputs left, outputs+events right** is correct for left-to-right data flow.
- **Shadow-based state indication** (glow halos for executed/cached/errored/missing) is visually distinctive — more elegant than badge-based approaches in traditional DCCs.
- **Viewport culling** is already implemented (`is_rect_visible` check).
- **Galley caching** (only rebuild on text/scale change) is a good optimization.

### Improvements

- **No LOD rendering** — at very low zoom, port circles and labels could be skipped, rendering only colored rectangles. Houdini does this.
- **No spatial indexing** for port hit-testing — fine for <500 nodes, becomes a bottleneck above that.
- **Execution time heatmap** — Houdini colors nodes by relative execution time (red=slow, green=fast). More powerful than absolute numbers alone.
- **Execution time should be toggleable** — timing labels add noise.
- **20-char name limit** in details panel is arbitrary and restrictive. Professional tools have no such limit.
- **Details panel fixed 250px** — should be resizable.

### Accessibility

- **Port radius 5px** (10px diameter) is below WCAG AA minimum of 24px for UI targets. The dynamic `port_activation_radius = port_row_height * 0.5` partially compensates but may still fall short at default scale.
- **Recommendation:** Add floor: `port_activation_radius = (port_row_height * 0.5).max(12.0 * scale)` to ensure at least 24px.
- **Remove button 10px** is also below WCAG minimum.
- **No keyboard navigation** for node/port interaction.
- **Noninteractive text** `#8C8C8C` on node background `#282828` is at ~4.2:1 contrast — borderline WCAG AA. Bump to `#999999` for safety.

---

## Style & Theming

### Current Implementation: Well-Structured

- **Two-level architecture** (StyleSettings → Style) is clean separation of persistence and runtime.
- **TOML format** is correct choice for Rust ecosystem. Custom hex color serde is well-implemented.
- **Scale reconstruction** on change is appropriate — rare operation, avoids per-frame multiplication.
- **Dark-mode only** is industry standard for creative/DCC tools.
- **Port colors** all pass WCAG AA contrast (3:1+) against graph background.
- **`#[serde(default)]`** for forward compatibility is best practice.

### Minor Improvements

- **Spacing scale** (5, 4, 2) doesn't follow standard base-2/base-4 grid. Consider aligning to (8, 4, 2) if visually acceptable.
- **Font sizes** (18, 15, 13, 12) roughly follow Major Third (1.25) scale from base 12 — adequate for dense technical UI.
- **Error shadow could be more prominent** than other status shadows (e.g., spread: 3 instead of 2).
- Port colors distinguish by **direction** (input/output), not by **data type** (Blender/Unreal model). Current approach is valid for fewer data types but consider data-type coloring if types diversify.
- Consider adding **shape differentiation** for port types (circle for data, diamond for trigger, square for event) alongside color — helps color-blind users.

---

## Background Grid

### Current Implementation: Functional

- **Tiled texture approach** with UV repeat is efficient for egui.
- **Power-of-2 scale snapping** (`wrap_scale_multiplier`) matches Blender's approach.

### Improvements

- **Hard snap between density levels** — Blender uses cubic alpha fading between levels. Add smooth alpha transition to eliminate jarring visual pop.
- **Single grid level** — professional editors show major/minor dots (heavier dots every 4x or 8x spacing). Two simultaneous levels with independent alpha.
- **Small base texture** (24x24) — at high zoom mipmap levels, the dot may vanish. Use 64x64+ for better mipmap quality, or regenerate on zoom threshold.
- **Dots-only** — some editors (Unreal) use grid lines. Consider making style configurable (dots/lines/hybrid). Blender's switch to dots-only generated user pushback.

---

## Log / Status Panel

### Current Implementation: Minimal

- Collapsible panel, last line when collapsed, 6 lines when expanded.
- No structured logging, no severity levels, no colors, no search.

### What Professional Editors Have

- **Structured entries** with severity levels (info/warning/error) — Nuke, Houdini, Substance
- **Color-coded by severity** — universal convention (gray info, yellow warning, red error)
- **Click error to select node** — Nuke double-click, Houdini/Substance badges on nodes
- **Virtual scrolling** for performance — `ScrollArea::show_rows()` in egui
- **Resizable panel** — draggable divider, not fixed line count
- **Severity filter toggles** — show/hide by level
- **Message deduplication** — Substance Designer shows "(# times)" suffix

### Recommended Upgrade Path

1. Structured `Vec<LogEntry>` with level enum + optional `node_id` (replaces raw string)
2. Color-coding by severity
3. Click entry → select and focus associated node
4. Virtual scrolling via `show_rows()`
5. Resizable panel height
6. Error/warning badges on nodes in graph view

---

## Undo/Redo

### Current Implementation: Solid

- Command pattern with inverse operations (`GraphUiAction` with before/after state) is textbook correct.
- Action coalescing for `NodeMoved` and `ZoomPanChanged` is exactly what professional editors do.
- Immediate vs. deferred classification is well-designed.

### Considerations

- **ZoomPanChanged in undo** — Blender does NOT make viewport changes undoable. Zoom/pan in undo history clutters navigation when users want to undo graph modifications. Consider removing.
- **Compound operations** — when multi-select exists, deleting 5 nodes should be one undo step. Need explicit group boundaries.
- **Undo stack size limit** — check if history grows unbounded. Professional editors cap at 100-500 steps.

---

## Auto-Layout

Not currently implemented beyond simple 3-column grid placement.

**Industry approach:** Sugiyama/layered layout algorithm for DAGs (the graph is already acyclic). Available in Rust via `dagre-rs`. Houdini offers A+drag for subtree layout, L for full layout. Should be a manual action (keyboard shortcut or button), not automatic.

**Priority: LOW** — useful for complex graphs but not a blocker.

---

## Priority Summary

| Feature | Priority | Effort | Impact |
|---------|----------|--------|--------|
| Multi-select (box select, shift-click) | CRITICAL | High | Very High |
| Search in new-node popup | HIGH | Low-Med | High |
| Keyboard shortcuts (delete, undo, copy/paste) | HIGH | Medium | High |
| Connection type checking | HIGH | Medium | High |
| Node collapse/expand | HIGH | Medium | High |
| Comment boxes / frames | MEDIUM | Medium | Medium |
| Search in graph (Ctrl+F) | MEDIUM | Medium | Medium |
| Node-type color coding | MEDIUM | Low | Medium |
| Reroute/dot nodes | MEDIUM | Low | Medium |
| Background alpha fading between zoom levels | MEDIUM | Low | Medium |
| Structured log entries + severity colors | MEDIUM | Medium | Medium |
| Animate fit-all/view-selected transitions | LOW | Low | Low |
| Auto-layout (Sugiyama) | LOW | High | Medium |
| Minimap | LOW | Medium | Low |
| Grid snap | LOW | Low | Low |
