# `stacking/combine` — correctness review against reference implementations

Scope: `lumos/src/stacking/combine/**` production code — `stack`, `rejection`, `normalization`,
`cache`, `config`, `pixel_coverage`. Test bodies were read only to establish coverage.

Checked against Siril's actual rejection source (`src/stacking/rejection_float.c`, read in full),
Siril's and PixInsight's published algorithm descriptions, Rosner's GESD critical-value definition,
and the standard Deming-regression and uniform-order-statistic results. Numeric claims below were
reproduced in `f32` Rust or Python; the reproductions are quoted inline.

Findings only — each item describes what is there. **Delete an item once you have addressed it.**

## Verdict

The engineering is unusually careful and most of the mathematics is right: the GESD critical values
match Rosner's formula exactly, the Deming slope (including its cancellation-free branch) matches
the textbook estimator with the noise ratio the right way round, the Winsorized constants match
Siril's source line for line, the quantization-noise median factors are the correct uniform
order-statistic results, and the compacted-index contract that pairs survivors with weights is
consistent everywhere it is relied on — including the one place it would break, which is explicitly
gated.

Against that: **finding 1 is a first-order defect on the default code path.** The sigma-clip fast
path declares a pixel clean, and skips rejection entirely, whenever two or more comparable outliers
land on the same pixel in a stack of ~10–24 frames — the archetypal satellite trail, cosmic ray or
surviving hot pixel. It reproduces 100% of the time for two 30σ outliers in a 10-frame stack. Every
existing test of that screen uses outliers of wildly different magnitudes, which is exactly the case
it handles correctly.

Findings 2–5 are places where a method under-rejects relative to the reference implementation it
names. Findings 6–16 are precision, budget and interpretation issues.

## 1. The sigma-clip fast path skips rejection that the exact loop would perform

`SigmaClipConfig::no_outliers_possible` is a screen that returns "nothing here can be rejected", and
`SigmaClipConfig::reject` trusts it in two places: before the sort (line 82) and again on the
shrunken window inside the iteration loop (line 101). It is not a conservative bound on the test it
stands in for.

- [ ] The exact test is `|v − median| ≤ k·1.4826·MAD`. The screen tests
      `max(|max−μ_trim|, |min−μ_trim|) ≤ k·σ_trim`, where `μ_trim`/`σ_trim` exclude **only the single
      most extreme sample from each end**. With two or more outliers on the same side, one survives
      the trim: it drags `μ_trim` toward itself *and* inflates `σ_trim`, and both effects push the
      comparison toward "clean". The two estimators are not related by any inequality once the data
      is contaminated, so the screen is not a superset of the exact keep-band.

- [ ] Reproduction, exact `f32`, transcribed from `sigma_clip_config.rs` and run standalone
      (`k = 2.5`, `max_iterations = 3`): eight background samples near 0.1 with ~0.005 spread, plus
      two samples at 0.5.

      ```text
      n = 10, screen says 'no outliers possible' = true
      exact loop survivors = 8  (the eight background samples)
      median = 0.1, sigma = 0.007413, outliers sit at 54.0 sigma
      stacked value with the screen    = 0.180000
      stacked value without the screen = 0.100000
      ```

      The pixel is off by 80% and no rejection is recorded.

- [ ] Failure map — fraction of random stacks (N(0,1) background, `nout` identical outliers at +Aσ,
      4000 trials per cell) where the screen says clean *and* the exact loop rejects:

      ```text
        n\A       3s       5s      10s      30s     100s
      -- 2 outliers
         10    19.0%    64.1%    99.3%   100.0%   100.0%
         12    17.8%    21.9%     0.0%     0.0%     0.0%
         16    13.2%     0.8%     0.0%     0.0%     0.0%
         24     7.3%     0.0%     0.0%     0.0%     0.0%
      -- 3 outliers
         10     5.1%    29.1%    90.6%   100.0%   100.0%
         16    20.4%    79.1%   100.0%   100.0%   100.0%
         20    27.2%    67.6%     0.0%     0.0%     0.0%
      -- 4 outliers
         16     8.1%    54.4%    99.8%   100.0%   100.0%
         24    23.3%    91.5%   100.0%   100.0%   100.0%
         48    30.0%     0.0%     0.0%     0.0%     0.0%
      ```

      A single outlier is safe (≤1.1%, and 0% past 5σ) — the trim was designed for that case. Two or
      more is not.

- [ ] This is the default path. `Rejection::default()`, `StackConfig::default()`,
      `StackConfig::light()` and `StackConfig::flat()` all resolve to `SigmaClip`, and the screen's
      `n < 10` guard means it is live for every stack of 10 frames or more.

- [ ] The in-loop call (line 101) has the same defect: after one iteration has removed the single
      most extreme sample, a remaining pair can stop the loop early.

- [ ] Cost of the sound alternative: the exact test is O(n) without a sort — `select_nth_unstable`
      for the median, then `mad_f32_fast` (a second selection over the deviations), then one pass
      comparing `min`/`max` deviation against `k·1.4826·MAD`. That is two selections against the
      current single pass, but it still skips the O(n log n) sort on clean pixels, which is what the
      screen exists for, and it cannot disagree with the loop it guards because it *is* the loop's
      first iteration.

## 2. Winsorized clipping rejects once; Siril and PixInsight iterate to convergence

`WinsorizedClipConfig::reject` runs `robust_estimate` and then a single `compact_within`. The type
has no `max_iterations` field at all — the 50-iteration cap inside `robust_estimate` bounds only the
inner σ-convergence loop, not the reject-and-re-estimate cycle.

- [ ] Siril's `WINSORIZED` case is an outer `do { … } while (changed && N > 3)`: after rejecting, it
      recomputes the median and re-runs the whole Winsorization on the survivors. PixInsight's
      description of the method is likewise iterative.

- [ ] One pass estimates `(center, σ)` from data that still contains every outlier. Winsorization
      caps their leverage at Huber's `c = 1.5`, which is what makes one pass usable — but with
      several outliers the capped mass still inflates σ, the first cut is looser than it should be,
      and there is no second pass to tighten it.

- [ ] `StackConfig::bias()`, `StackConfig::dark()` and `StackConfig::winsorized()` all use it, all
      with `SmallN::none()`, so it is the sole rejector at any frame count on the calibration path.

## 3. `PercentileClipConfig` is not the percentile clipping of PixInsight or Siril

Same name, different algorithm, and a parameter whose numbers mean something else.

- [ ] Siril, verbatim from `rejection_float.c`:

      ```c
      if (median - pixel > median * plow)   { rej[0]++; return -1; }
      else if (pixel - median > median * phigh) { rej[1]++; return 1; }
      ```

      That is a **relative-deviation-from-the-median** test: `plow`/`phigh` are fractions of the
      median value, not fractions of the sample count. PixInsight's percentile clipping is described
      the same way.

- [ ] `PercentileClipConfig::surviving_range` drops `floor(p/100 · n)` samples from each end of the
      sorted stack. That is a symmetric trimmed mean — IRAF's `minmax` rejection expressed as
      fractions. A perfectly good estimator, and genuinely the right tool for 3–6 frame stacks; it
      is just not what the name says.

- [ ] The units diverge too: this takes 0.0–50.0 as a percent of samples, the references take a
      fraction of the median (Siril's default `p_low = p_high = 0.2`, i.e. 20% of the median value).
      A user carrying `0.2` across gets a 0.2%-of-samples trim, which for any stack under 500 frames
      rejects nothing at all.

## 4. `LinearFit` with `max_iterations = 1` never fits a line

`LinearFitClipConfig::reject` spends `iteration == 0` on a median+MAD clip and only starts fitting at
`iteration == 1`.

- [ ] `validate_max_iterations` accepts `1`, so `LinearFitClipConfig::new(σ, σ, 1)` validates and
      silently degrades to a plain (and slightly different) sigma clip, under a name that promises a
      gradient-tolerant fit.

- [ ] `Rejection::linear_fit()` hard-codes `3`, so the preset is safe; a hand-built config or a
      deserialized one is not.

- [ ] Siril's `LINEARFIT` has no seed pass — it fits from the first iteration and loops
      `while (changed && N > 3)`. The seed pass here is a defensible addition (it is what keeps the
      non-robust OLS fit below from being dragged by gross outliers), but it costs one of only three
      iterations.

## 5. Fixed iteration caps where the references iterate to convergence

- [ ] `SigmaClipConfig` and `LinearFitClipConfig` default to `max_iterations = 3`. Siril's `SIGMA`,
      `WINSORIZED` and `LINEARFIT` all loop until a pass rejects nothing (`while (changed && N > 3)`);
      PixInsight iterates likewise. Three passes is usually enough, but on a pixel where each pass
      exposes the next outlier — a trail crossing several frames — the last ones survive into the
      mean.

- [ ] `SigmaClipConfig::reject` already detects convergence (`if new_lo == lo && new_hi == hi break`),
      so raising the cap costs nothing on clean pixels; the cap is only load-bearing on the pixels
      that are still rejecting.

- [ ] Siril also refuses to reject below ~4 survivors (`if (N - r <= 4)`). Here the floor is
      `len <= 2`, so a 3-sample window can be cut to one survivor in a single pass. The default
      `SmallN::median_below(5)` covers the whole-stack case but not a pixel that is down to three
      frames through partial coverage.

## 6. Normalization's per-thread sample buffers are outside the memory budget

- [ ] `measure_common_stats` does `.map_init(|| Vec::with_capacity(common_domain.sample_count), …)`
      over `frames × channels`. Rayon builds one such buffer **per worker thread**. At 6144×6144 with
      full common coverage that is 37.7 M samples ≈ 151 MB each; on a 32-thread machine ≈ 4.8 GB, on
      top of the frames the loader budgeted for.

- [ ] It is the registered-lights path — precisely when the spill tier was chosen because memory was
      already tight — and `cancellable_median_mad` then sorts in place, so the peak is real.

- [ ] `load_budget_is_respected_across_configs` models frames only; the neighbouring test's own
      comment says as much ("invisible to […] which models frames rather than scratch").

- [ ] `gather_valid_samples` and `gather_indexed_samples` both call `plane.chunk(0, pixel_count)` —
      the whole plane — from inside a `par_iter`, so the mmap tier faults in as many full planes at
      once as there are threads, ignoring the chunk sizing entirely.

## 7. `linear_variance` is not a variance once `Weighting::Noise` is on

- [ ] `CombinedSample::from_survivors` computes `Σw²/(Σw)²`, and `StackProduct::linear_variance`
      documents exactly that. It is the variance of the weighted mean **only when every frame shares
      one σ**. Under `Weighting::Noise` the true variance is `Σ(wᵢσᵢ)²/(Σwᵢ)²`, and with
      `wᵢ ∝ 1/σᵢ²` that collapses to `1/Σwᵢ_raw` — a different plane.

- [ ] The mission statement asks for a product a downstream tool can put error bars on. A consumer
      multiplying this plane by a representative σ² gets the wrong answer for the one weighting mode
      designed to improve SNR.

- [ ] The missing term is already in hand: `FrameStats::channels[c].mad` and the normalization gain
      are both available at the same place `resolve_weights` reads them, so folding `(gain·σ)²` into
      the per-survivor accumulation costs one extra multiply per survivor and makes the plane an
      actual variance.

## 8. Rejection centres are the upper-middle order statistic, not the median

- [ ] `SigmaClipConfig` uses `active[len / 2]`, `WinsorizedClipConfig` uses `working[mid]`, and
      `LinearFitClipConfig`'s seed pass uses `median_f32_fast` — all the upper-middle element. For
      even N that is biased high: at n = 10 the expected offset is +0.123σ, at n = 20 +0.087σ.

- [ ] A symmetric band `center ± kσ` then sits high, so the **low** side clips harder than the high
      side. For an astro stack that is backwards — satellites, cosmic rays and aircraft are all
      high-side events, and the asymmetric constructors exist precisely to clip the high side harder.

- [ ] `sorted_mad` is measured about the same shifted centre, which inflates it slightly and
      partially masks the effect, but does not cancel it.

- [ ] The median **combine** is correct: `run_stacking` calls `median_f32_mut`, which averages the
      two middle values for even N. So the bias is confined to the rejection centres, and the two
      conventions coexist in one module without either doc saying so.

## 9. GESD's automatic outlier cap is far below the references, worst exactly where the preset turns it on

- [ ] `GesdConfig::max_outliers_for_size` is `(n/4).min(2 if n < 25 else 10)`. Siril's default is
      `0.3·n` — 4× more at n = 50, 15× more at n = 100.

- [ ] `StackConfig::gesd()` sets `SmallN::median_below(15)`, so GESD first runs at 15 frames — where
      the cap is **2**. A 15–24 frame stack can never reject more than two samples at a pixel, which
      is less than sigma clipping or percentile clipping would remove. The band 15–24 is the one
      range where the method is enabled and nearly powerless.

- [ ] The comment cites "Rosner's validated limits". Rosner validated the *critical values* for
      n ≥ 25 with r ≤ 10; that is a statement about the accuracy of the λ table, not a ceiling on how
      many outliers a pixel may have. Capping the *test* at 2 and falling back to nothing is a
      stronger reading than the source supports.

- [ ] `validate()` does not reject `max_outliers: Some(0)`, which silently disables rejection.

- [ ] `alpha: 0.0` passes validation (`(0.0..1.0).contains`), giving `inverse_cdf(1.0)`. The limiting
      critical value is finite and correct — `(m−1)/√m` — but the test that covers it uses
      `f32::MIN_POSITIVE`, so whether `statrs` returns `+∞` or panics at exactly `1.0` is untested.

## 10. `k` means a different thing in each rejector, behind one shared `SigmaBounds`

`SigmaBounds` documents itself as "in units of the spread estimate", and the three methods that share
it calibrate that spread differently.

- [ ] `SigmaClip` and `Winsorized` are in Gaussian σ (MAD × 1.4826; the Winsorized sd carries the
      1.134 bias correction). `LinearFit`'s spread is the mean absolute residual about a line fitted
      to the **sorted** stack — which shrinks as N grows, because sorted samples hug the quantile
      ramp ever more closely.

- [ ] Measured false-rejection rate on clean N(0,1) data, per sample:

      ```text
      n=10   LinearFit k=3.0 -> 2.8%    SigmaClip k=2.5 -> 5.0%
      n=20   LinearFit k=3.0 -> 4.5%    SigmaClip k=2.5 -> 3.7%
      n=40   LinearFit k=3.0 -> 5.9%    SigmaClip k=2.5 -> 2.7%
      ```

      They cross over. LinearFit's detection power is correspondingly much higher (a 3σ outlier at
      n = 40 is caught 99.3% of the time against sigma clipping's 77.5%), so this is a real trade
      rather than a bug — Siril behaves the same way, and recommends larger σ for linear fit for
      exactly this reason. The problem is that nothing here says so, and a user reads one `sigma`
      field across all three.

## 11. "Sigma clipping" here is MAD-based; the references' method of that name is not

- [ ] Siril's `SIGMA` case uses `siril_stats_float_sd` — the standard deviation. Its MAD variant is a
      separate menu entry (`MAD` clipping), and it compares against the *raw* MAD without the 1.4826
      rescale. PixInsight's and `astropy.stats.sigma_clip`'s defaults are likewise the standard
      deviation, with MAD available as an option.

- [ ] `SigmaClipConfig` uses `mad_to_sigma(MAD)` throughout. That is the more robust choice and
      arguably the right one — but it is strictly tighter than an sd-based cut whenever outliers are
      present, which is when it matters. A σ = 2.5 carried over from a PixInsight workflow rejects
      more here. The doc comment says "kappa-sigma clipping" without saying which spread estimator,
      so the difference is invisible at the call site.

## 12. The Deming fit's inlier window may exclude the pixels that constrain the slope

- [ ] `paired_photometric_gain` seeds with a MAD-ratio gain, then keeps only residuals inside
      `4 · 1.4826 · MAD(residual)` of the residual median. The residual MAD over a stratified sample
      of the common domain is sky-noise dominated.

- [ ] Any star bright enough to have a photometric lever arm has a residual far outside that window
      as soon as the seed gain is even slightly off — a 5% mismatch at 0.5 in normalized units is
      0.025, against a window of a few times the sky σ. So the Deming fit runs on what is left, which
      is close to flat sky, where `s_xy` is small and the slope is poorly determined.

- [ ] The `covariance <= f64::EPSILON → 1.0` guard means a genuinely flat field degrades to unity
      gain rather than to a wild one, so this is a precision question, not a crash. Worth measuring:
      on a real registered set, compare the Deming gain against the MAD-ratio seed and against the
      unregistered `Normalization::Global` path. If they disagree by more than the noise, the window
      is the first thing to look at.

## 13. Smaller precision and interpretation items

- [ ] **`Weighting::Noise` collapses the channels.** `resolve_weights` averages `gain·1.4826·MAD`
      across channels into one `avg_sigma`, then applies `1/avg_sigma²` to every channel.
      PixInsight computes noise weights per channel. For a colour set where one channel is much
      noisier (narrowband, or a heavily light-polluted blue), the shared weight is optimal for
      neither.

- [ ] **`quantization_sigma` uses channel 0's gain for every channel.** Both
      `SourceSigmas::combined_mean` and `::conservative` read `norms[index].channels[0].gain`, and
      `StackProduct` carries one scalar for the whole product. Fine for CFA masters (one plane), an
      approximation for RGB with per-channel normalization.

- [ ] **The median combine silently drops warp confidence.** `warn_if_weights_ignored` fires only for
      a non-`Equal` `Weighting`. A registered median stack also discards the per-pixel confidence
      multiplier — inherent to the median, but nothing says so at the call site or in the warning.

- [ ] **`LinearFit`'s residual scale divides by `n`, not `n − 2`.** The line has two fitted
      parameters, so the spread is biased low by `√(n/(n−2))` — 5% at n = 10. Siril does the same, so
      this is a match rather than a divergence; noting it because the module's stated priority is
      precision over parity.

- [ ] **`WinsorizedClipConfig::robust_estimate` centres its sd on the median, not the mean.** Siril
      and PixInsight both take the standard deviation of the Winsorized set about its **mean**, and
      the 1.134 constant is calibrated for that. Measured impact is small — σ runs 1.6–5% high across
      clean and contaminated cases, and rejection counts were identical in every case tried — so this
      is a documentation item rather than a defect, but the 1.134 comment should say the centre was
      changed.

- [ ] **The first Winsorization window is 13% wide.** Siril seeds the loop with the plain sd;
      `robust_estimate` seeds it with `sd · 1.134`. It converges to the same place, just from
      further out.

## Verified correct

Checked in detail and found right — recorded so a later reader does not re-derive them:

- **GESD critical values.** With `m = live_count`, the code computes
  `(m−1)/√(m·(1 + (m−2)/t²))`, which is algebraically identical to Rosner's
  `λ = t·(m−1)/√((m−2+t²)·m)`, and `p = 1 − α/(2m)` with `df = m−2` matches the standard
  definition term for term. The decision rule (largest `i` with `Rᵢ > λᵢ`, via `rposition`) is
  Rosner's, not the naive "first failure" rule.
- **GESD's reverse-Welford removal.** `SS ← SS − (x − μ_old)(x − μ_new)` with
  `μ_new = μ_old − (x − μ_old)/(n−1)` is the exact downdate, in `f64`, over at most 10 steps.
- **GESD's survivor compaction.** Candidates are swapped to the tail in removal order, so the
  `original_len − num_outliers` prefix is exactly the survivor set even when fewer candidates are
  confirmed than were tested. Indices are swapped in lockstep.
- **`sorted_mad`.** The two-pointer merge over `[center − sorted[l−1]]` descending-left and
  `[sorted[r] − center]` ascending-right does yield the absolute deviations in global ascending
  order, and stopping at rank `m/2` reproduces `median_f32_fast` of the deviations exactly.
- **The contiguous-window sigma clip.** On sorted data `center − kσ_low ≤ v ≤ center + kσ_high` is a
  contiguous run, and the two `partition_point` bounds implement the same inclusive/exclusive
  convention as `within_threshold`. Equivalent to per-element compaction, at O(log n) per iteration.
- **Deming regression.** `δ = σ²_reference/σ²_frame` is the right orientation for `y = reference`,
  `x = frame`; the slope matches the standard estimator; the `delta < 0` branch is the exact
  cancellation-free rearrangement `2δ·s_xy/(root − delta)`; and the `δ → ∞` limit correctly reduces
  to OLS of reference on frame.
- **Winsorized constants.** `c = 1.5`, `1.134`, and the `0.0005` relative convergence threshold are
  Siril's, character for character (`sigma = 1.134f * siril_stats_float_sd(w_stack, N, NULL)`;
  `while (fabsf(sigma - sigma0) > sigma0 * 0.0005f)`). The 50-iteration cap is an improvement —
  Siril's inner loop is unbounded.
- **Linear-fit clipping.** OLS through `(sorted index, value)` with the spread as mean absolute
  residual, rejecting each sample against its own fitted value, is Siril's `LINEARFIT` exactly.
- **Quantization-noise median factors.** `√(3n/((n+1)(n+2)))` for even n and `√(3/(n+2))` for odd n
  are the correct standard deviations of the sample median of n uniform (quantization-error)
  variates, both derived from the order-statistic covariances and both matching the convention
  `median_f32_mut` actually uses (average of the two middle values for even n).
- **Quadrature propagation.** `√Σ(wᵢ·gainᵢ·σᵢ)²/Σwᵢ` is right for independent sources under a
  weighted mean, and the `MaxSigma` seed-from-"nothing rejected" is the correct floor.
- **The compacted-index contract.** `scratch.indices` holds indices into the *compacted* per-pixel
  arrays, not frame indices. `weighted_mean_indexed` and `CombinedSample::from_survivors` both index
  `eff_weights[..covered]`, which is consistent. The one place that needs true frame indices —
  `SourceSigmas::combined_mean` in `run_stacking` — is gated behind `frame_indices_are_stable`,
  which requires every frame to carry no coverage. Correct, and the only subtle thing in the module
  that is genuinely load-bearing.
- **Normalization is applied before rejection**, inside the gather loop, so rejection asks about
  photometrically comparable values. Matches PixInsight and Siril.
- **`stratified_valid_indices`** returns exactly `min(sample_count, 65536)` indices —
  `floor(k·S/R)` is strictly increasing in `k` for `R ≤ S`, so each target rank is hit once.
- **`Weighting::Noise` includes the `gain²` term**, so a frame scaled up to match the reference is
  not over-weighted. That is the "pscale²" correction, and it is easy to omit.
- **`run_stacking` asserts the cache's normalization equals the config's**, closing the one way a
  reused cache could silently apply the wrong photometric scale.
- **Cancellation** leaves zeros and reports `Error::Cancelled` through a single exit
  (`finish_unless_cancelled`), so a partial stack cannot be mistaken for a whole one.
- **`PixelCoverage`** is one rule, read by the combine, the coverage plane and the common domain, so
  the three cannot describe different frame sets.

## Coverage gaps in the tests

- [ ] Nothing cross-checks `no_outliers_possible` against the loop it guards. The six tests that
      exercise it are hand-computed single cases, all with outliers of very different magnitudes —
      the case it handles. A property test over random stacks asserting *"screen true ⟹ exact loop
      rejects nothing"* finds finding 1 in seconds.
- [ ] More generally: no rejector has a "fast path agrees with slow path" test. Every method with an
      early exit (`sigma < EPSILON`, `no_outliers_possible`, `new_lo == lo && new_hi == hi`,
      `write_idx == len`) deserves one.
- [ ] No test that a second Winsorized pass would reject nothing further — which is the assertion
      that would justify the single pass in finding 2.
- [ ] No test pins percentile clipping's semantics against a stated definition, so finding 3 reads as
      intentional from inside the module.
- [ ] No test that `LinearFitClipConfig` with `max_iterations = 1` performs a linear fit.
- [ ] Nothing covers the even-N centre convention, in either direction — neither that the rejection
      centre is the upper-middle nor that the median combine averages.
- [ ] The memory tests model frame residency only; the normalization scratch of finding 6 is
      unmodelled and unmeasured.
- [ ] `Weighting::Noise` has no test that a noisier frame actually receives less weight *after*
      normalization gain is applied (the `gain²` term).

## Gaps against the reference feature sets

Not defects — recorded so the scope decision is explicit.

- [ ] **No rejection maps.** PixInsight outputs per-pixel low/high rejection counts, which is the
      standard way to tell whether σ was set sanely. `weight` and `coverage` do not answer it: a
      pixel that rejected three frames and one that never had them read the same.
- [ ] **No large-scale pixel rejection.** PixInsight's two-stage rejection catches extended
      structures (large satellite trails, plane tracks) that per-pixel statistics treat as signal.
- [ ] **No local normalization.** Only one global affine per frame per channel, so a gradient that
      moves between frames is normalized on average and rejected as outliers locally.

## Sources

Read directly:

- Siril rejection implementation, `src/stacking/rejection_float.c` —
  <https://gitlab.com/free-astro/siril/-/raw/master/src/stacking/rejection_float.c>
- [Rejection algorithms (Siril 1.0)](https://free-astro.org/siril_doc-en/co/Average_Stacking_With_Rejection__1.html)
- [Stacking — Siril documentation](https://siril.readthedocs.io/en/stable/preprocessing/stacking.html)
- [Rosner's Test for Outliers — EnvStats](https://alexkowa.github.io/EnvStats/reference/rosnerTest.html)
  and [Rosner's Outlier Test — PNNL VSP](https://vsp.pnnl.gov/help/vsample/rosners_outlier_test.htm)

Read via search summary (the sites refuse direct fetches):

- [Image Integration — Question about Winsorized Sigma Clipping, PixInsight Forum](https://pixinsight.com/forum/index.php?threads/image-integration-question-about-winsorized-sigma-clipping.1558/)
  — Huber's procedure as PixInsight implements it, `c = 1.5`, mean and standard deviation of the
  Winsorized set.
- [A detailed look into PixelRejection](https://dslr-astrophotography.com/detailed-pixel-rejection-methods/)
  and [PixInsight Image Integration](https://chaoticnebula.com/pixinsight-image-integration/) —
  percentile clipping's role and recommended frame counts.

`PixInsight/PCL` does not ship the `ImageIntegration` module source publicly, and
`pixinsight.com/doc/tools/ImageIntegration/` returns 403 to automated fetches, so PixInsight
comparisons above rest on its published descriptions rather than on its code. Siril comparisons rest
on its source.
