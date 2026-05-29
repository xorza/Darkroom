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
| 5 | Stacking & drizzle (rejection, weighting, F&H) | [`05-stacking-drizzle.md`](05-stacking-drizzle.md) | 1126 |

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

## Findings flagged against lumos source (for review — not changed)

While grounding the research, the agents read lumos source and flagged these. Each
is a *claim to verify*, with a pointer; none has been changed (research/docs only).

| Stage | Finding | Pointer |
|-------|---------|---------|
| 3 | **`Star.roundness1`/`roundness2` are swapped vs the DAOFIND/photutils convention — CONFIRMED** with quoted Stetson 1987 + photutils definitions. lumos also drops the ×2, uses a marginal-max not a Gaussian fit, computes both on the unconvolved stamp, and its SROUND is a clamped asymmetry-RMS. Highest-value item. | `src/star_detection/star.rs:24`, `centroid/mod.rs:666` |
| 4 | **`warp()` emits no coverage/footprint mask** for extrapolated pixels — they should be down-weighted in the combine. Biggest stacking-correctness gap. | `src/registration/` warp |
| 4 | lumos's robust loss is a bespoke kernel, **not** standard MAGSAC++ ρ — worth deciding if that's intended (works in tests, but diverges numerically from the paper). | `src/registration/ransac/magsac.rs` |
| 5 | GESD uses an inverse-**normal** approximation instead of Student-t critical values → over-rejects at small N; no output **variance/weight map**; no blot/drizzle-CR; scalar-only normalization; noise weighting omits a `pscale²` term. | `src/stacking/rejection.rs:759`, `src/stacking/` |
| 3 | Background uses median/MAD only — no Pearson `2.5·median−1.5·mean` mode estimator (with the `|mean−median|/σ<0.3` switch), no tile-grid median filter. | `src/star_detection/background/` |
| 1 | Load-time clamp to `[0,1]` can positively bias dark/bias **masters**; integer-FITS `BLANK` unhandled; only `primary_hdu()` read (no multi-extension/tile-compressed FITS); CFA/Bayer-drizzle pieces exist but unwired. | `src/raw/mod.rs`, `src/astro_image/fits.rs` |
| 2 | No dark **scaling/optimization**; no single-frame cosmic-ray rejection; no bad-column/overscan/superbias; no uncertainty plane. | `src/calibration_masters/mod.rs` |
| 4 | SIP order fixed (not auto-selected); `Auto` upgrade skips the Euclidean/Affine rungs; TPS implemented but unwired. | `src/registration/` |

## Remaining unverifiable / caveated claims (much reduced after pass 2)

- **Bertin & Arnouts 1996** (SExtractor): the only ADS copy is a scanned image with
  no text layer — covered via the SExtractor source, the readthedocs manual, and
  the "for Dummies" guide rather than the primary text.
- **Umeyama 1991** (similarity Procrustes): all mirrors are scanned — the det(R)=+1
  result is cross-checked via the lumos code and the standard formula.
- **RCD**: no formal paper exists; the algorithm is defined as code in
  `librtprocess/rcd.cc` (treated as authoritative).
- The **Winsorization 1.134** unbiasing constant isn't stated in a primary stats
  reference (it propagates as a shared PixInsight/Siril/lumos constant; derivation
  sketched as ≈1/√0.778 for a Huber c=1.5 break).
- Small-N median efficiency (~0.74 at N=3) is quoted from standard order-statistics
  theory, not a parsed table.

## Regenerating

- Reference source clones: `scripts/clone-refs.sh` (`--all` for the large suites),
  persisting in `.tmp/refs/`.
- Parsed papers: `.tmp/papers/*.txt` (extracted with `pdftotext`; `brew install poppler`).
- Both `.tmp/` subtrees are gitignored.
