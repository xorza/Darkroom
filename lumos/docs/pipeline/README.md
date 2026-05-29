# Lumos Pipeline — Best-Practices Reference

Deep reference documentation on best practices, state-of-the-art algorithms, and
anti-patterns for each stage of the lumos astrophotography pipeline. Each document
was built from two evidence bases:

- **Upstream source code** cloned under `.tmp/refs/` (see `scripts/clone-refs.sh`) —
  LibRaw, librtprocess, RawTherapee, cfitsio, ccdproc, SExtractor, SEP, photutils,
  PSFEx, astroalign, MAGSAC++, astrometry.net, SCAMP, SWarp, reproject, the STScI
  drizzle/DrizzlePac C core, Siril, DeepSkyStacker, and OpenCV.
- **Web research**, with each load-bearing claim cross-checked against ≥2 sources
  (FITS Standard 4.0; the SExtractor, DAOFIND, MAGSAC++, SIP, and Fruchter & Hook
  papers; vendor docs for PixInsight / Siril / DeepSkyStacker).

These are *reference* documents — they describe how the field does each stage well,
and what to avoid — not a prescriptive change list for lumos. The per-document
"How lumos currently does it" sections compare against lumos source for context.

## Documents

| # | Stage | Document | Lines |
|---|-------|----------|-------|
| 1 | Load & decode (FITS, RAW, demosaic) | [`01-load-decode.md`](01-load-decode.md) | 689 |
| 2 | Calibration (bias/dark/flat, defects) | [`02-calibration.md`](02-calibration.md) | 673 |
| 3 | Star detection (background→deblend→centroid) | [`03-star-detection.md`](03-star-detection.md) | 760 |
| 4 | Registration (match→RANSAC→SIP→warp) | [`04-registration.md`](04-registration.md) | 870 |
| 5 | Stacking & drizzle (rejection, weighting, F&H) | [`05-stacking-drizzle.md`](05-stacking-drizzle.md) | 842 |

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

## Findings flagged against lumos source (for review — not yet verified by maintainer)

While grounding the research, the agents read lumos source and flagged the
following. Each is a *claim to verify*, with a pointer; none has been changed.

| Stage | Finding | Pointer |
|-------|---------|---------|
| 1 | Load-time clamp to `[0,1]` can positively bias dark/bias **masters** (signed residuals lost). | `src/raw/mod.rs` load path |
| 1 | Integer-FITS `BLANK` not handled (only float NaN/Inf sanitized); float-FITS divide-by-max is per-file/lossy. | `src/astro_image/fits.rs` |
| 1 | Have both pieces for CFA/Bayer-drizzle but it is not wired; no superpixel/split-CFA mode. | `src/raw/`, `src/drizzle/` |
| 2 | No dark **scaling/optimization** (k=1 only); no single-frame cosmic-ray rejection (relies on stack-time clipping); no bad-column or overscan/superbias handling. | `src/calibration_masters/mod.rs` |
| 3 | **`Star.roundness1`/`roundness2` appear swapped vs the DAOFIND/photutils convention**, and are computed on the unconvolved stamp. Highest-value item to confirm. | `src/star_detection/star.rs:24`, `centroid/mod.rs:666` |
| 3 | Background uses median/MAD only — no Pearson `2.5·median−1.5·mean` mode estimator, no crowding switch, no tile-grid median filter. | `src/star_detection/background/` |
| 3 | Deblend contrast is measured relative to the **parent node** flux, not SExtractor's **root/total** flux. | `src/star_detection/deblend/multi_threshold/mod.rs:811` |
| 4 | **`warp()` emits no coverage/footprint mask** for extrapolated pixels — the biggest stacking-correctness gap (those pixels should be down-weighted in the combine). | `src/registration/` warp |
| 4 | SIP order is fixed (not auto-selected); `Auto` upgrade skips the Euclidean/Affine rungs; TPS implemented but unwired. | `src/registration/` |
| 5 | GESD uses an inverse-**normal** approximation rather than Student-t critical values → over-rejects at small N; no output **variance/weight map**; no blot/drizzle-CR; scalar-only normalization (no surface/background match for mosaics); noise weighting omits a `pscale²` term. | `src/stacking/rejection.rs:759`, `src/stacking/` |

## Notable unverifiable / caveated claims

The agents flagged a few claims they could not pin to a primary source (all
corroborated indirectly, but worth knowing):

- **Markesteijn X-Trans**: behaviour/tradeoffs confirmed, but no authoritative
  step-by-step algorithmic walkthrough was found in prose — relies on the
  RawTherapee/librtprocess source.
- **Fruchter & Hook (2002)** and **Stetson (1987, DAOFIND)** PDFs would not parse
  via fetch; their equations were reconstructed from the STScI C source / photutils
  and corroborated against ≥2 secondary sources (the drizzle update equations and
  the correlated-noise `R = r/(1−r/3)` formula agree across source + docs).
- **DeepSkyStacker** `theory.htm` refused direct fetch; its dark-scaling and
  hot-pixel rules were corroborated via DSS source and search snippets.

## Regenerating

The clone script lives at `scripts/clone-refs.sh` (`--all` for the large suites).
Sources persist in `.tmp/refs/` (gitignored) across sessions.
