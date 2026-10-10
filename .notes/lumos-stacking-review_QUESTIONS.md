# Questions from executing `lumos-stacking-review.md`

Each entry names the plan item it blocks, the options, a recommendation, and what waits on the
answer. The plan keeps the blocked part with a pointer here.

## Q1. X-Trans cosmic-ray significance eats well-sampled stars (Batch 13, CAL-7 and CAL-9)

**Finding.** On an X-Trans mosaic the single-frame cosmic-ray pass flags the whole of a star, at
every size tried (Gaussian σ 0.8 to 4 px, peak 0.5 over a sky of 0.05, noise 0.003), at nine
sub-pixel positions, even with `objlim` raised from 5 to 40. Mono L.A.Cosmic keeps every one of
those stars at `objlim` 5. This is not caused by the Batch 13 changes; it was there before.

**Cause.** The X-Trans significance is `L⁺ = max(0, v − median of the 8 nearest same-colour
neighbours)`. That is not a second difference. On a star's convex wings it is positive, because
the nearest same-colour neighbours of a red or blue photosite lie on one side more than the other,
and there the fine structure `median₈ − median₂₄` is near zero. The contrast test passes in the
wings, the wing pixels become primary detections, and the growth step, which tests no contrast,
spreads from them into the core over the iterations (217 pixels of one σ = 3 star). Mono's
`L⁺` is a clipped Laplacian, which is zero on convex wings.

A symmetric second difference is not available for red and blue: on the Fujifilm layout no
red or blue photosite has a point-symmetric same-colour pair nearer than √18 ≈ 4.2 px, too coarse
for stars of FWHM 3 to 6 px. Greens have symmetric pairs at √2.

No reference tool runs L.A.Cosmic on an X-Trans mosaic, so there is no established statistic to
copy.

**Options.**

| Option | What it does | Cost |
|---|---|---|
| A. New X-Trans statistic | Replace `v − median` by the residual of a robust local plane fitted to the same-colour neighbours (zero on a linear slope, negative on convex wings, positive at a peak), and calibrate `objlim` against mono on Gaussian stars | A design and a calibration with no reference to check against; medium work; risk of a statistic that is still wrong in some phase |
| B. Refuse X-Trans in this pass | `reject_cosmic_rays` returns a typed error for an X-Trans frame, as the module doc already steers colour data to stack-time rejection | Removes a feature `lens` may expose; X-Trans users then rely on stack-time rejection only |
| C. Keep it | No change | Stars are eaten on every X-Trans light this pass runs on: quietly wrong output |

**Recommendation.** B now, and A later as its own item if single-frame rejection on X-Trans is
wanted: B is correct today, A is research.

**Blocked.** The X-Trans part of CAL-7 (`objlim` calibration and the star-preservation test) and
the X-Trans part of CAL-9 (local recompute). The rest of Batch 13 is done: the saturated-star mask
(CAL-6), the σ normalization of the X-Trans significance and its noise-only test (CAL-7), the
detector types (CAL-17), and the mono and Bayer local recompute (CAL-9).
