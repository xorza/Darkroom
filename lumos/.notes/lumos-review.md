# lumos review

Findings only — each item describes what is there, not what to do about it. **Delete an item once you
have addressed it**; this file lists open findings and nothing else. No "done" markers, no history.

Items are anchored to symbol names rather than line numbers, which go stale within a session.

Scope: `lumos/src` production code. Test bodies and the APIs tests reach through were not reviewed.

---

## The thin-plate-spline module is 1400 lines nothing calls, and solves its system twice

`registration/distortion/tps/` carries `#![allow(dead_code)]` with the note "no code outside this
module (or its tests) calls TPS yet" — accurate, and the rule about deliberately-kept code is
satisfied. Recording it because of the size and because of what is inside it.

- [ ] 384 production lines plus 1027 test lines, reachable only from its own tests: `mod tps` is
      private in `distortion/mod.rs` and nothing there re-exports it. The tests exercise it
      thoroughly, so this is 1400 lines that prove only their own consistency.
- [ ] `ThinPlateSpline::fit` solves the *same* matrix twice, once per right-hand side, so two O(n³)
      eliminations are paid where one factorization reused for both would do. Costs nothing today
      because nothing runs it.

### Plan

The size item decides the other one, so settle it first: **delete, or integrate**. The double solve
is only worth fixing in the integrate case, and is not the first thing to fix there.

**Option A — delete it.** Nothing depends on it, and git has it if it is ever wanted. The mission in
`lumos/CLAUDE.md` points here: "Anything that adds significant machinery without serving either the
image or its measurability is out of scope and should be removed rather than carried." SIP already
holds the distortion role, is the FITS-standard convention, and is what the comparable tools use.

**Option B — integrate as a post-RANSAC distortion option**, in this order:

1. **The warp path first, because it is the blocker.** TPS cannot be applied per pixel as written.
   `WarpTransform::apply` runs once per output pixel whenever `is_linear()` is false (`row`'s
   `for_each_source_position` can only step incrementally for a linear transform), and
   `ThinPlateSpline::transform` is O(control points) with a `sqrt` and a `ln` each — at 500 control
   points on a 6144×4096 frame that is ~1.3e10 transcendental evaluations per channel. The module
   already contains the answer: `DistortionMap::{from_tps, interpolate}` samples the spline onto a
   grid and interpolates it bilinearly in O(1) per pixel. Integration has to go through that, and the
   grid spacing has to be chosen against a *measured* bound on grid-vs-exact-spline residual in px.
2. **One distortion sum type across the public surface.** `Config::sip`, `RegistrationResult::sip_fit`,
   `WarpTransform::sip` + its `apply`, and `RegistrationResult::warp_transform` are all typed to SIP
   concretely. A parallel `Option<TpsFit>` beside each would admit "both models at once", which
   neither supports — this wants `DistortionModel::{None, Sip(..), Tps(..)}` replacing the `Option`
   in all four places.
3. **Fit diagnostics to match `SipFitResult`** (rms, max residual, points used/rejected) so the two
   models can be compared on the same numbers when choosing between them.
4. **Then the double solve.** Add a factor-once/solve-many pair to `math::linear_system`:
   `factor_in_place(a, pivots)` recording the row swaps, and `solve_factored(a, pivots, b)` doing the
   two substitutions. Scale: n = control points + 3, so at 500 control points one elimination is
   ~4e7 flops (tens of ms) and halving it is worth having — once TPS runs, and not before.

**Option C — keep as documented WIP.** Costs nothing at runtime; the standing cost is that 1400 lines
must keep compiling and 1027 test lines must keep passing.

Recommendation: **A**, unless there is a real frame set that SIP at order 5 cannot correct — that
evidence is what would justify B, and it should be produced before any of B is built.

Note for whoever does step 4: the same factor-twice pattern is *live* in SIP today —
`sip::solve_masked` runs `solve_cholesky` on the same `A^T·A` once per axis. Leave it alone on
performance grounds: n ≤ 18 there, so a factorization is under a thousand flops and the saving is
unmeasurable. If the factor/solve split lands for TPS, SIP can adopt it for tidiness only.
