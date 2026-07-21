# Lumos Pipeline — Best-Practices Reference

Deep reference documentation on best practices, state-of-the-art algorithms, and
anti-patterns for each stage of the lumos astrophotography pipeline. Built and then
**validated across two research passes** against two evidence bases:

- **Upstream source code** cloned under `.tmp/refs/` (see `scripts/clone-refs.sh`) —
  LibRaw, librtprocess, RawTherapee, cfitsio, ccdproc, SExtractor, SEP, photutils,
  PSFEx, astroalign, MAGSAC++, astrometry.net, SCAMP, SWarp, reproject, the STScI
  drizzle/DrizzlePac C core, Siril, DeepSkyStacker, and OpenCV.
- **Primary literature**, with 27 papers downloaded and text-extracted into
  `.tmp/papers/` (via `pdftotext`) — incl. the FITS Standard 4.0, Bertin & Arnouts
  1996 (SExtractor), Stetson 1987 (DAOFIND), Fruchter & Hook 2002 + Casertano 2000
  (drizzle), MAGSAC/MAGSAC++, astrometry.net, Groth 1986, the SIP convention,
  Hartley 1997, van Dokkum 2001 (L.A.Cosmic), Malvar 2004, and the NIST GESD test.
  Every load-bearing claim is cross-checked against ≥2 sources.

These are *reference* documents — how the field does each stage well, and what to
avoid — not a prescriptive change list. The per-document "How lumos currently does
it" sections compare against lumos source for context.

## Documents

| # | Stage | Document | Lines |
|---|-------|----------|-------|
| 1 | Load & decode (FITS, RAW, demosaic) | [`01-load-decode.md`](01-load-decode.md) | 1089 |
| 2 | Calibration (bias/dark/flat, defects) | [`02-calibration.md`](02-calibration.md) | 1034 |
| 3 | Star detection (background→deblend→centroid) | [`03-star-detection.md`](03-star-detection.md) | 1023 |
| 4 | Registration (match→RANSAC→SIP→warp) | [`04-registration.md`](04-registration.md) | 1128 |
| 5 | Stacking & drizzle (rejection, weighting, F&H) | [`05-stacking-drizzle.md`](05-stacking-drizzle.md) | 1268 |

Each doc ends with a `## Primary sources parsed (pass 2)` subsection mapping each
extracted PDF to a one-line takeaway and its local `.tmp/papers/<name>.txt`.

## Cross-cutting principle: stay linear, stay calibrated

Every stage shares one rule that distinguishes astrophotography from terrestrial
imaging: **operations must preserve flux linearity and not clip signal** so that
downstream photometry, rejection statistics, and drizzle remain valid. The most
common anti-patterns across all five stages are violations of this:

- nonlinear white balance / gamma / sRGB color management applied to linear data (Stage 1);
- clamping pixels to `[0,1]` (or to ≥0) before all signed corrections are done (Stages 1–2);
- demosaicing *before* calibration / defect correction (Stages 1–2);
- a hard inlier threshold or nearest-neighbour resampling on science pixels (Stage 4);
- pixel rejection without prior normalization, or sigma-clipping with too few frames (Stage 5).

## Corrections made in pass 2 (errors found in pass 1)

The second validation pass — armed with the primary PDFs — overturned several
pass-1 claims. These are now fixed in the docs (marked `**Correction (pass 2):**`):

| Stage | What pass 1 got wrong | Corrected to |
|-------|----------------------|--------------|
| 1 | RCD green-channel formula given as `G·(1+(LPF₀−LPF₂)/(LPF₀+LPF₂))` | The real directional ratio estimates `N_Est = cfa·2·lpf₀/(eps+lpf₀+lpf_N)` with gradient-weighted V/H blending (per `librtprocess/rcd.cc`) |
| 1 | Markesteijn "3-pass" mischaracterized | It doubles direction count (4→8) and re-derives green per pass; homogeneity selection runs once |
| 3 | "All DAOFIND stats are computed on the convolved image" | GROUND (roundness) is computed on the **unconvolved** image; only sharpness/SROUND use convolved data |
| 3 | Deblend contrast measured against parent-node flux | SExtractor measures it against **root/total isophotal flux** (`fdflux`) |
| 4 | lumos implements the MAGSAC++ closed-form ρ "specialized to k=2 via γ(1,x)=1−e⁻ˣ" | **Wrong** — lumos's loss is a *bespoke MAGSAC++-inspired* kernel, not the paper's ρ for any DoF (numerically: paper-n4 ρ(3)=0.861 vs lumos 0.519) |
| 5 | Correlated-noise `R = r/(1−r/3)` (from the DrizzlePac Handbook) | F&H Eq. 10 has numerator **1**: `R = 1/(1−r/3)` — the Handbook's printed form is a typo (fails the r→0 ⇒ R→1 sanity check) |
| 5 | "Smaller pixfrac inflates correlated noise"; weight `W = Σa·w·s²` | R is monotonically **increasing** in `r=p/s` (small pixfrac's real cost is coverage/holes); the output-scale `s²` lives only in the flux numerator, never in the weight (F&H Eqs. 2–3) |

## Findings flagged against lumos source

While grounding the research, the source comparison flagged these items. The table records current
status; resolved rows remain here so the research documents' historical references have context.

| Stage | Finding | Pointer |
|-------|---------|---------|
| 3 | **Resolved:** `Star.roundness1` is GROUND and `roundness2` is SROUND; measurement constructs the canonical `Star` directly and filtering preserves one nested diagnostics record. | `star_detection/centroid/mod.rs`, `detector/stages/` |
| 4 | **Resolved:** `warp()` returns `WarpResult { image, coverage, confidence }`; coverage gates geometric support while normalized coefficient energy determines the independent confidence multiplier. | `registration/resample/`, `combine/cache/mod.rs` |
| 4 | lumos's robust loss is a bespoke kernel, **not** standard MAGSAC++ ρ — worth deciding if that's intended (works in tests, but diverges numerically from the paper). | `src/registration/ransac/magsac.rs` |
| 5 | **Resolved:** GESD now uses Rosner's two-sided mean/sample-sd statistic, the live sample count, and an accurate Student-t inverse; the preset falls back to median below 15 frames. Post-rejection channel-shaped WHT and optional linear-variance images are implemented; median correctly exposes no linear factor. Registered stacks preserve pre-warp source noise, fit normalization on paired common-sky samples, and apply interpolation confidence once. Remaining gaps are no input variance model, blot/drizzle-CR, or surface normalization; drizzle intentionally omits F&H's global `s²` flux factor and preserves input DN scale. | `stacking/combine/{normalization,rejection}/`, `stacking/drizzle/` |
| 3 | **Resolved:** background tiles use the crowding-aware Pearson-mode switch plus a 3×3 tile-grid median filter and natural spline interpolation. | `background_mesh/`, `star_detection/background/` |
| 1 | Load-time clamp to `[0,1]` can positively bias dark/bias **masters**; CFA/Bayer-drizzle pieces exist but are unwired. | `src/io/raw/mod.rs`, `src/io/image/fits.rs` |
| 2 | Dark scaling/optimization, bad-column/overscan/superbias, and an uncertainty plane remain absent. Single-frame L.A.Cosmic is implemented for mono, Bayer, and X-Trans CFA layouts and runs before demosaic when configured. | `calibration_masters/cosmic_ray.rs`, `pipeline/streaming.rs` |
| 4 | SIP order remains caller-selected. `Auto` walks Euclidean → Similarity → Affine → Homography; TPS remains implemented and intentionally reserved for later integration. | `registration/mod.rs`, `registration/distortion/` |

## Remaining unverifiable / caveated claims (much reduced after pass 2)

- **Bertin & Arnouts 1996** (SExtractor): the only ADS copy is a scanned image with
  no text layer — covered via the SExtractor source, the readthedocs manual, and
  the "for Dummies" guide rather than the primary text.
- **Umeyama 1991** (similarity Procrustes): all mirrors are scanned — the det(R)=+1
  result is cross-checked via the lumos code and the standard formula.
- **RCD**: no formal paper exists; the algorithm is defined as code in
  `librtprocess/rcd.cc` (treated as authoritative).
- ~~The **Winsorization 1.134** constant…~~ **Resolved (pass 3):** verified by direct
  integration — `E[ψ_{1.5}²] = 0.77846`, `1/√0.77846 = 1.13339 ≈ 1.134` (stage 5, §3.2).
- ~~Small-N median efficiency (~0.74 at N=3)…~~ **Corrected (pass 3):** computed exactly
  — var-eff `0.743` at N=3 (SD `0.862`), *decreasing* to `2/π` as N→∞; the limit is
  approached from *above*, so the prior "worse penalty at small N" was backwards
  (stage 5, §2.1).

## Regenerating

- Reference source clones: `scripts/clone-refs.sh` (`--all` for the large suites),
  persisting in `.tmp/refs/`.
- Parsed papers: `.tmp/papers/*.txt` (extracted with `pdftotext`; `brew install poppler`).
- Both `.tmp/` subtrees are gitignored.
