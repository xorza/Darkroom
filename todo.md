I have enough. The annotated assembly tells the story cleanly for all three.

---

1. Shape::is_noop — IPC 0.22

    5.76 │ push %rbx ; function-entry skid
    0.00 │ mov (%rdi),%eax ; load enum tag @ offset 0
    16.63 │ lea -0x1(%rax),%rcx ; ← skid of preceding load
    0.00 │ xor %edx,%edx
    0.00 │ cmp $0x2,%rax
    2.45 │ cmovae %rcx,%rdx
    6.15 │ lea -0x64a010(%rip),%rcx ; jump-table addr (rodata)
    0.00 │ movslq (%rcx,%rdx,4),%rdx ; load jump-table entry
    45.77 │ add %rcx,%rdx ; ← skid of jump-table load
    13.87 │ jmp \*%rdx ; indirect dispatch

Two stalls per call, both confirmed by skid pattern (cost lands on the instruction after the load):

- 16.6% — cold load of the variant tag at [rdi] (Shape is heap/arena-backed, just-built by caller but not always hot).
- 45.8% — jump-table load + 13.9% indirect jump = ~60% of all cycles in this function are an unpredictable indirect branch. The jump table
  itself is in .rodata and always L1-hot; what hurts is the BTB miss on the variant dispatch.

Why Brush::is_noop (next door, same shape of code) gets IPC 4.03: Brush dispatch is dominated by Solid in real UI workloads — predictor
learns it. Shape dispatch sees an interleaved mix of RoundedRect, Text, Polyline, Line per widget — predictor can't lock on. Plus
Brush::is_noop is called from Shape::is_noop, so when it runs the Brush bytes are already cache-hot.

The kicker is at forest/shapes/mod.rs:70:
if shape.is_noop() { return None; }
...
let record = match shape { // ← second dispatch on same tag
Shape::RoundedRect { .. } => { ... }
...
};
Two indirect dispatches on the same tag, back-to-back. The compiler can't fuse them because is_noop is a separate function (and the match
shape consumes the value).

Fix: kill the standalone is_noop and fold the no-op check into each arm of the match shape in Shapes::add. One dispatch, predictor sees
one pattern, the post-dispatch arm already has the variant fields decoded. Quick code-level win.

---

2. Text::show — IPC 1.13

    2.62 │ movzbl 0x98(%rsi),%eax ; load self.wrap / self.align byte
    ...
    0.06 │ mov 0x90(%r15),%rcx ; load self.style discriminant (or InternedStr field)
    44.94 │ mov %rcx,0x20(%rsp) ; ← skid of preceding load
    5.25 │ vmovups 0x80(%r15),%xmm0 ; load 16 bytes from offset 0x80
    0.04 │ vmovaps %xmm0,0x10(%rsp)
    ...
    0.48 │ vmovss 0x10(%rsp),%xmm0
    6.51 │ vmulss 0x24(%rsp),%xmm0,%xmm0
    11.75 │ vmovss %xmm0,0xc(%rsp) ; ← skid of vmulss FP latency

Text struct layout:
pub struct Text {
element: Element, // ← big, ~0x80 bytes given the read offsets
text: InternedStr, // at offset ~0x80
style: Option<TextStyle>, // at offset ~0x90
wrap: TextWrap, // at offset ~0x98
align: Align, // at offset ~0x99
}

The 44.94% sample is the skid for loading self.style (offset 0x90) — sitting in the second cache line of a Text passed by value through a
fat builder stack frame. The build_ui closure constructs dozens of Texts and Buttons in the same stack frame; older Texts have already
evicted their lines by the time show runs.

Element is enormous (salt, mode, mode_payload, sizes×3, padding, margin, gaps, justify, align×2, position, grid, flags, visibility,
transform). The text-specific fields all live after byte 0x80. So every Text::show call eats a cold-line miss to read style/wrap/align.

Fix options, easy → involved:

- Reorder fields: put text/style/wrap/align before element in the struct. Same size, but the small text fields share a line with the Text
  builder's header (Element fields read by node() get a separate fetch they'd take anyway).
- Or: style: Option<TextStyle> is 5×f32 + tag — already inline. If most callers don't set it (None), it's a wasted 24+ bytes per builder.
  Shrink to Option<&TextStyle> (8 bytes, pointer into theme).
- Or: Text::show could prefetch &self.style after the first make_persistent_id work. But field reorder is cheaper and gets all widgets.

Same pattern is going to be true for Button::show, Panel::show, Frame::show — anything with element: Element first. Worth measuring
whether reorder helps all of them; Button is 2.7% cyc at IPC 3.44 already, so the issue is most concentrated on Text whose own work after
the loads is small (just one add_shape call).

---

3. Tree::close_node (IPC 1.22) vs Tree::post_record (IPC 4.32)

close_node source touches 5 SoA columns indexed by node id:

let i = closing.idx();
let shapes = &mut self.records.shape_span_mut()[i]; // col @ self+0x?
let end = self.records.subtree_end()[i]; // col @ self+0x128
if self.records.layout()[i].mode == LayoutMode::Grid { // col @ self+0x130/0x138
self.has_grid.insert(i); // bitset @ self+0x158
}
let i_has_grid = self.has_grid.contains(i);
if let Some(parent) = scratch.open_frames.last() { ... ends[pi] ... } // again

Annotated hot loads, all clustering on skid-after-load:

21.08 │ sub $0x28,%rsp ; entry skid — caller-call latency
0.14 │ mov 0x10(%rsi),%rcx ; load scratch.open_frames.len
...
0.57 │ mov 0x150(%rdi),%rsi ; load len of subtree_end col
19.85 │ cmp %rax,%rsi ; ← skid: len bound-check load
...
0.00 │ mov 0x130(%rdi),%rsi ; load layout col base
0.00 │ mov 0x138(%rdi),%r10 ; load layout col …
0.00 │ mov (%rsi,%rax,4),%r9d ; indexed load layout[i]
14.59 │ lea (%rax,%rax,8),%rsi ; ← skid of layout[i] load
...
0.14 │ cmpb $0x7,0x1b(%r10,%rsi,1) ; load tag byte from layout struct
...
0.00 │ mov 0x158(%rdi),%rsi ; has_grid bitset base
15.07 │ mov %eax,%r10d ; ← skid of bitset load

Three to four indexed loads, all on different cache lines, all at non-sequential i (close order = LIFO of recently-opened nodes, jumps
around). The prefetcher can't help — it learns from sequential strides.

Compare post_record (IPC 4.32): walks indices 0..n linearly, the L1 hardware prefetcher stays one cache line ahead on every column it
touches. Same code shape, totally different access pattern.

Fix options:

- Drop has_grid as a separate bitset. It's read twice per close (once via contains(i), once for insert(pi)), each touching a different
  bit. The mode == Grid info is already in self.records.layout()[i].mode which is already loaded in the same function. Replace
  has_grid.contains(i) with re-reading self.records.layout()[i].mode == Grid — but for the subtree propagation, you do need a propagated
  bit. Could be packed into a spare bit of subtree_end or layout (which we just loaded anyway), saving one cache line per call.
- Inline close_node into the record-time } close path so the caller, which just touched layout[i] etc. milliseconds ago for the current
  node, still has those lines hot.
- Bigger lift: split the SoA columns into "touched at open", "touched at close", "touched at post" buckets and group the close-touched
  ones into one struct-of-arrays cache line per node. Right now each node's close-time data spans 3-4 cache lines.

---

TL;DR — pick-up order

┌───────────────────────────────────────────────────────────┬─────────────────────────────────────────────┬──────────────────────────┐
│ Fix │ Expected payoff │ Code risk │
├───────────────────────────────────────────────────────────┼─────────────────────────────────────────────┼──────────────────────────┤
│ Fold Shape::is_noop into the match shape in Shapes::add │ high — frees ~1% of total frame and lifts │ trivial │
│ (eliminate the double dispatch) │ that block from IPC 0.22 → ~2 │ │
├───────────────────────────────────────────────────────────┼─────────────────────────────────────────────┼──────────────────────────┤
│ Shrink Option<TextStyle> → Option<&TextStyle> and/or │ ~0.5–1% frame; lifts Text::show IPC 1.13 → │ low; one-line struct │
│ reorder Text fields so text/style/wrap/align come before │ ~3; same trick benefits Panel/Frame/Button │ edit + a constructor │
│ element │ │ tweak │
├───────────────────────────────────────────────────────────┼─────────────────────────────────────────────┼──────────────────────────┤
│ Replace has_grid bitset by a packed bit inside layout[i] │ ~0.3–0.5% frame on close_node │ medium — need to find a │
│ │ │ free bit in LayoutCore │
└───────────────────────────────────────────────────────────┴─────────────────────────────────────────────┴──────────────────────────┘

The libc 0.04-IPC memcpy line stays as-is — it's bandwidth-bound, not code-bound; only way to shrink it is to copy less (lower volume from
the FrameArena lowering callers).
