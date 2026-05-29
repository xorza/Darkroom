# Stage 1 — Load & Decode: Best Practices & Algorithms

Reference material for the `lumos` astro-image library. This document covers the
first stage of an astrophotography pipeline: reading FITS files and camera RAW
files, decoding sensor data, and (when applicable) demosaicing a color-filter-array
sensor — producing a **linear, planar, floating-point** image that downstream
calibration, registration, stacking, and photometry can trust.

Claims here are cross-checked against the FITS Standard 4.0, the cloned upstream
sources under `.tmp/refs/` (LibRaw, librtprocess, RawTherapee, cfitsio, astropy),
and the published astrophotography/demosaicing literature. Where authoritative
sources disagree, that is called out explicitly.

---

## Scope & Goal

Stage 1 turns bytes on disk into a `[0, 1]`-normalized, **linear** (proportional
to collected photons) floating-point image stored **planar** — one channel buffer
per color — with enough metadata to validate that downstream calibration is still
mathematically meaningful.

The hard requirements, in priority order:

1. **Linearity must be preserved.** Every operation in this stage must be an affine
   per-pixel map (subtract a constant, multiply by a constant) or a *linear*
   interpolation of neighboring pixels. The moment a gamma curve, tone curve,
   log stretch, or saturation/“vibrance” adjustment is applied, the relationship
   `pixel ∝ photons` is destroyed and flat-fielding, stacking statistics, and
   photometry all become invalid.
2. **Bit-exact integer handling.** Integer FITS and RAW values must be reconstructed
   exactly (correct `BZERO`/`BSCALE`, correct black level) before any float math.
   A 1-DN error at the black point biases every subsequent statistic.
3. **Calibration validity must be protected.** For one-shot-color (OSC) sensors the
   single most important rule is: **calibrate the raw CFA mosaic, then demosaic** —
   never the reverse. Demosaicing mixes neighboring photosites; doing it first
   smears hot pixels, breaks the per-photosite dark/bias model, and makes flat
   division per-channel-incorrect.
4. **Planar f32 output.** Channels are stored as separate contiguous buffers
   (`common::Buffer2<f32>` in lumos), which is the natural layout for per-channel
   SIMD, per-channel statistics, and channel-independent registration/stacking.

Everything below serves those four goals.

---

## 1. FITS loading

FITS is the lingua franca of calibrated astronomical data and the output of every
serious capture/processing tool (N.I.N.A., SharpCap, MaxIm DL, PixInsight,
DeepSkyStacker, Siril, ASCOM drivers). The image is an N-dimensional array of one
pixel type, preceded by an ASCII header of 80-byte `KEYWORD = value / comment`
cards. The pitfalls are almost all in *value reconstruction* and *axis/orientation
conventions*, not in the array read itself.

### 1.1 BITPIX, BZERO/BSCALE — the integer→float contract

`BITPIX` declares the on-disk sample type: `8` (unsigned byte), `16` (signed
16-bit), `32` (signed 32-bit), `64` (signed 64-bit), `-32` (IEEE float),
`-64` (IEEE double). The physical value is reconstructed by the FITS Standard 4.0
scaling equation:

```
physical_value = BZERO + BSCALE × array_value
```

with defaults `BZERO = 0.0`, `BSCALE = 1.0` (FITS Standard 4.0 §4.4.2; verified at
<https://heasarc.gsfc.nasa.gov/docs/fcg/standard_dict.html> and the GSFC floating-point
agreement <https://fits.gsfc.nasa.gov/fp89.txt>).

**The unsigned-16 convention is the critical landmine.** FITS has no native
unsigned integer type. The universal convention (astropy applies it *by default*) is
to store `uint16` data as `BITPIX = 16` (signed) with `BZERO = 32768`, `BSCALE = 1`.
The value `0` is stored on disk as `-32768`; the reader must add 32768 to recover it.
For `uint32` the shift is `BZERO = 2147483648`. Any loader that ignores `BZERO`
reads an unsigned-16 light frame as a signed-16 array centered on zero — half the
pixels go negative and the histogram is destroyed. Sources:
astropy docs <https://docs.astropy.org/en/stable/io/fits/usage/image.html> ("int16
data with BZERO=32768 and BSCALE=1 would be treated as uint16 data") and the
STScI reserved-keywords guide.

**Best practice:** never read the raw array and apply scaling yourself unless you
are certain of the keyword semantics. Let cfitsio do it. cfitsio's `fits_read_pix`
/ `fits_read_img` family applies `BZERO`/`BSCALE` transparently and, when you request
output as `TUSHORT`/`TUINT`, recognizes the unsigned conventions automatically
(`.tmp/refs/cfitsio/` — the `imcompress.c`/`getcolumn` machinery; the public contract
is documented in the cfitsio user guide). The Rust binding `rust-fitsio`
(`.tmp/refs/rust-fitsio/`) wraps this; reading into `Vec<f32>` (as lumos does)
delegates the integer→float promotion and scaling to cfitsio. The cost is that the
*loader cannot see* whether `BZERO` was the unsigned shift or a genuine physical
offset — which matters for the float-normalization heuristic below.

### 1.2 BLANK and NaN — undefined pixels

For **integer** images, undefined pixels are flagged by the `BLANK` keyword: any
array value equal to `BLANK` is "undefined" and should not be treated as data
(FITS Standard 4.0 §4.4.2.5). `BLANK` is only meaningful for integer `BITPIX` and
**must be ignored for floating-point images** — there, an IEEE NaN *is* the blank
indicator (verified across the GSFC standard dictionary and the floating-point
agreement). A correct reader therefore:

- integer image: map array values equal to `BLANK` to a sentinel / mask before
  any statistic;
- float image: treat NaN (and, defensively, ±Inf — which the standard does not
  define as data) as undefined.

A subtle trap: after `BZERO`/`BSCALE` scaling, the test for `BLANK` must be done
against the *raw array value*, not the scaled value, because a scaled `BLANK` may
not be exactly representable. cfitsio handles this internally when you set a null
value via `fits_read_imgnull` / `fits_set_imgnull`. If you read scaled floats and
then test for NaN (lumos's approach), you correctly catch float blanks but
**silently miss integer `BLANK` pixels**, because the integer `BLANK` value scales
to an ordinary finite float. This is a real gap for data from instruments that
write integer `BLANK` (uncommon in amateur OSC/CMOS data, common in survey/archival
integer FITS).

### 1.3 Endianness

FITS arrays are **big-endian** ("most significant byte first") by definition
(FITS Standard 4.0 §3.3.2). Byte swapping on little-endian hosts (x86, ARM) is
mandatory and is handled inside cfitsio; never read FITS bytes raw. This is a
non-issue if you go through cfitsio/rust-fitsio and a guaranteed corruption bug if
you don't.

### 1.4 Float FITS has no normalization convention

The FITS standard defines the integer→physical mapping but says nothing about the
*range* of float data — and explicitly recommends against using `BZERO`/`BSCALE`
with float images. Consequently float FITS in the wild lands in mutually
incompatible ranges:

- PixInsight writes `[0, 1]`;
- DeepSkyStacker writes `[0, 65535]`;
- some tools write `[0, 255]` or raw ADU / electrons / physical flux.

There is **no header keyword that disambiguates this**, so a loader must either
(a) carry the range as out-of-band knowledge, or (b) apply a heuristic. lumos uses
heuristic (b): if the data max exceeds a threshold (2.0), divide by the max.
This is pragmatic but lossy — it cannot tell `[0, 1.5]` HDR data from genuinely
small physical values, and dividing by the max is *not affine across files*
(two frames of the same target with different brightest pixels get different
scale factors, which breaks inter-frame normalization unless stacking re-normalizes,
which lumos's stacking stage does). The honest position: there is no fully correct
solution for headerless float FITS; document the assumed range and prefer carrying
it explicitly when the source is known.

### 1.5 Axis order and orientation

FITS stores axes in `NAXIS1, NAXIS2, NAXIS3` order where `NAXIS1` is the
fastest-varying (the image *width*/columns), `NAXIS2` is height/rows, and `NAXIS3`
(if present) is the channel/plane axis. cfitsio/rust-fitsio return the shape in
**reverse** (C/row-major) order, so a 2-D image comes back as `[height, width]` and
a 3-D image as `[channels, height, width]`. lumos handles this correctly in
`src/astro_image/fits.rs:49-58`. Color FITS is **planar** (all of plane 1, then all
of plane 2, …) — never interleaved — which matches the planar f32 target exactly.

The vertical-origin convention is the classic orientation trap. FITS pixel (1,1) is
conventionally the **lower-left** corner (the array's first row is the bottom of the
sky image), inherited from FORTRAN/astronomy. Most camera-control software writes
top-down, so the `ROWORDER` keyword (`TOP-DOWN` default, `BOTTOM-UP` otherwise)
disambiguates. This matters enormously for **CFA color assignment**: flipping the
row order vertically swaps which physical color sits at `(0,0)`, so the Bayer
pattern string must be flipped to match. lumos does this in `read_cfa_from_headers`
(`fits.rs:170-196`).

### 1.6 Bayer keywords for OSC FITS

OSC sensors saved as FITS carry their mosaic intact, with the pattern described by:

- `BAYERPAT` (a.k.a. `COLORTYP`): the 2×2 string `RGGB` / `BGGR` / `GRBG` / `GBRG`
  (some writers emit `TRUE` meaning RGGB).
- `XBAYROFF` / `YBAYROFF`: integer offsets of the readout origin into the CFA
  matrix. An **odd** X offset flips the pattern horizontally; an **odd** Y offset
  flips it vertically. This is how a sub-framed/cropped readout that starts on a
  different photosite is described without rewriting the pattern.
- `ROWORDER`: as above, a `BOTTOM-UP` value flips the pattern vertically.

These three corrections **compose** (each is an independent horizontal or vertical
flip of the 2×2 pattern), and getting any one wrong produces a debayered image with
swapped red/blue channels or a fatal "checkerboard" color maze. lumos composes all
three in `read_cfa_from_headers` (`fits.rs:170-196`) — `ROWORDER` then `YBAYROFF`
(both vertical) then `XBAYROFF` (horizontal). astropy's CCD reduction guide and the
SBFITSEXT / MaxIm-DL header conventions document these keywords; cross-checked at
<https://cdn.diffractionlimited.com/help/maximdl/FITS_File_Header_Definitions.htm>.

### 1.7 Common header pitfalls

- **Coordinate encodings vary**: RA/Dec may be decimal degrees (`RA`/`DEC`, N.I.N.A.,
  SGP), sexagesimal strings (`OBJCTRA`/`OBJCTDEC`, MaxIm/ASCOM, in *hours* for RA),
  or WCS reference points (`CRVAL1`/`CRVAL2`). RA in HMS must be multiplied by 15 to
  reach degrees. lumos handles the three forms in `read_ra_deg`/`read_dec_deg`.
- **Temperature/gain keyword aliases**: `CCD-TEMP` vs `CCDTEMP`, `EGAIN` (e⁻/ADU) vs
  `GAIN` (camera gain setting) — they are *not* interchangeable and feed different
  noise models.
- **`IMAGETYP` vs `FRAME`** for the light/dark/flat/bias label, which calibration
  matching depends on.
- **Trailing-space and case sloppiness** in string values (`'BOTTOM-UP '`,
  `'rggb'`) — compare case-insensitively and trimmed.

---

## 2. RAW decoding (the libraw model)

Camera RAW (CR2/CR3, NEF, ARW, RAF, DNG, …) is the other major input. The
reference implementation, and the one lumos wraps, is **LibRaw** (a maintained fork
of dcraw). The decode is a fixed pipeline; understanding each step is essential to
*not undo* libraw's work or double-apply a correction.

### 2.1 Unpack

`libraw_open_buffer` + `libraw_unpack` parse the container, decompress, and populate
`rawdata.raw_image` — the **full sensor area** including optical-black / masked
borders, as raw integer ADU. The active-image rectangle is given by
`sizes.{width,height}` inset by `sizes.{top,left}_margin` within
`sizes.raw_{width,height}`. lumos validates these in `open_raw`
(`src/raw/mod.rs:601-714`) and crops to the active area when extracting. **Do not**
demosaic or compute statistics over the masked border — those pixels are not sky.

### 2.2 Black-level subtraction — `adjust_bl()` and the per-channel model

This is the most error-prone part of RAW decoding, and the part most pipelines get
subtly wrong. Sensors have a nonzero "black" pedestal (the ADU of a zero-photon
pixel), and on modern CMOS it is **per-CFA-color** and sometimes **spatially
patterned**, not a single scalar.

LibRaw consolidates all of this in `adjust_bl()`
(`.tmp/refs/LibRaw/src/utils/utils_libraw.cpp:464-545`). The data structures:

- `C.black` — a scalar global pedestal.
- `C.cblack[0..3]` — per-channel pedestals for the 2×2 CFA positions (R, G1, B, G2).
- `C.cblack[4]`, `C.cblack[5]` — the dimensions of an *additional* spatial black
  pattern (e.g. a 6×6 repeating offset), with `cblack[6+]` holding that pattern.

`adjust_bl()` (and lumos's faithful port `consolidate_black_levels`,
`src/raw/mod.rs:91-207`) does, in order:

1. If the CFA is a 2×2 Bayer (`filters > 1000`) with a ~2×2 spatial pattern, fold the
   spatial `cblack[6+]` values into the four per-channel `cblack[0..3]` via the
   `FC(row,col)` color macro, remapping the second green to channel 3. For X-Trans /
   Fuji RAF-DNG (`filters <= 1000`, 1×1 pattern) fold `cblack[6]` into all four
   channels. Then zero `cblack[4]=cblack[5]=0`.
2. Extract the **common minimum** across the four per-channel values, subtract it
   from each, and add it to `C.black`. This separates "the part every channel
   shares" (a cheap uniform subtract) from "the per-channel delta."
3. If a residual spatial pattern remains, extract *its* minimum into `C.black` too.
4. Final per-channel black = `cblack[c] + black`.

The actual subtraction (`subtract_black_internal`,
`.tmp/refs/LibRaw/src/preprocessing/subtract_black.cpp:24-91`) then does
`val = CLIP(image[i][c] − cblack[c])` per pixel and updates the channel maximum.
`CLIP` clamps the result to `[0, 65535]` — so **libraw clips negative
post-black values to zero by default.**

lumos's port is structurally faithful but makes one deliberate optimization and one
deliberate divergence:

- *Optimization*: it splits the subtraction into a SIMD pass that removes the
  `common` pedestal uniformly (`normalize_u16_to_f32_parallel`, which also does the
  `1/(max−common)` scale in the same pass), then a second pass that applies the
  per-channel delta (`apply_channel_corrections`, `src/raw/mod.rs:240-262`). This is
  exactly libraw's "common + delta" decomposition, just executed in two vectorizable
  passes.
- *Divergence to watch*: lumos clamps each pixel to `[0, 1]` (`.max(0.0)…​.min(1.0)`).
  Clamping the bottom to 0 matches libraw's `CLIP`, **but for calibration frames this
  is not ideal** — dark and bias frames legitimately produce values that scatter
  *below* the pedestal (read noise is symmetric), and clamping them to zero injects a
  positive bias into the master frame. lumos partly mitigates this: `CfaImage::subtract`
  (`src/astro_image/cfa.rs:146-159`) deliberately *keeps* negatives during dark
  subtraction. But the initial `normalize` clamp at load time has already removed the
  sub-pedestal scatter for the raw frames themselves. (See §5 and §6.)

**Why per-channel matters:** if you subtract a single scalar black from a sensor
whose green channels sit 8 ADU higher than red, the residual offset survives into the
demosaic and shows up as a faint color cast that flat-fielding cannot remove (flats
correct *multiplicative* response, not *additive* offset). Photometry on such data
has a per-color zero-point error.

### 2.3 White balance / camera multipliers — and why astro keeps them near unity

LibRaw's `scale_colors()` (`.tmp/refs/LibRaw/src/postprocessing/postprocessing_utils_dcrdefs.cpp:106-249`)
computes the white-balance multipliers `pre_mul[4]` (from camera `cam_mul`,
auto-WB grey-world, or user values), subtracts black from the white level
(`maximum -= black`), and then forms `scale_mul[c] = (pre_mul[c]/dmax) × 65535/maximum`.
The decisive lines for astro are 195-207:

```c
for (dmin=DBL_MAX, dmax=c=0; c<4; c++) {       // dmin = min mul, dmax = max mul
    if (dmin > pre_mul[c]) dmin = pre_mul[c];
    if (dmax < pre_mul[c]) dmax = pre_mul[c];
}
if (!highlight) dmax = dmin;                    // <-- key line
if (dmax > 1e-5 && maximum > 0)
    FORC4 scale_mul[c] = (pre_mul[c] /= dmax) * 65535.0 / maximum;
```

When highlight clipping mode is *off* (`!highlight`, i.e. clip mode 0), libraw divides
**all** multipliers by `dmax = dmin` — the **smallest** multiplier becomes 1.0 and
every channel is multiplied by `≥ 1.0`. This guarantees no channel scales a valid
pixel *down*, so a near-saturated pixel in the weakest channel does not get pushed
above the white level and clipped. When highlight recovery is on, it normalizes by
`dmax` instead (multipliers `≤ 1.0`), trading headroom for highlight detail.

lumos mirrors the `!highlight` behavior precisely: `compute_wb_multipliers`
(`src/raw/mod.rs:213-234`) normalizes so the **minimum** multiplier is 1.0, and
applies WB only on the demosaic (light-frame) path — never on the calibration
(`extract_cfa_pixels`) path, where it passes `&[1.0; 4]`.

**Should astro apply WB at all?** Two defensible positions:

- *Keep WB unity (raw-linear).* Many astro pipelines (Siril's raw import, PixInsight's
  preference for raw mono channels) deliberately apply *no* WB at decode and let a
  later, physically-motivated color-calibration step (e.g. photometric color
  calibration against a star catalog, or simple background-neutralization) set the
  channel scaling. This keeps each channel's values proportional to that channel's
  electrons, which is exactly what per-channel flat division and noise modeling want.
- *Apply camera WB (min-normalized).* Applying the camera multipliers with the
  min-normalization above is still a per-channel *affine* (multiplicative) operation,
  so it does **not** break linearity, and it makes the channels roughly equal in scale
  for nicer previews. The risk is purely that an aggressive multiplier can clip
  highlights — which the min-normalization specifically avoids.

The thing astro must **never** do is libraw's full *post-processing* color pipeline:
`output_color != 0` (camera→sRGB/Adobe matrix), a nonlinear `gamm[]` curve, or
auto-brightness. Those are nonlinear and/or gamut-mapping and destroy photometry. When
lumos falls back to libraw's built-in demosaic for exotic CFAs it correctly forces
`gamm = {1,1}` (linear), `output_color = 0` (raw color, no matrix),
`no_auto_bright = 1`, `output_bps = 16` (`src/raw/mod.rs:477-491`).

### 2.4 White level / saturation and highlight handling

`color.maximum` is the per-camera white level (the ADU at which a photosite
saturates), and `data_maximum` is the brightest value actually present. The
normalization range is `maximum − black`, so a saturated photosite maps to exactly
1.0. lumos uses `1/(maximum − common)` as its scale (`consolidate_black_levels`,
`mod.rs:185-190`) and asserts the effective max is positive.

For astrophotography, **highlight recovery is undesirable and should be off.**
Highlight reconstruction (dcraw/libraw modes ≥ 2, or PixInsight/RT "highlight
recovery") *invents* data in clipped channels by borrowing from unclipped ones — a
nonlinear, non-physical guess. Saturated stars must instead be *detected and excluded*
from photometry (lumos's star detector flags `is_saturated`), not reconstructed.
Clipping the top to 1.0 is acceptable *as a flag of saturation*; what matters is that
the saturation level is known so those pixels can be masked, not silently trusted.

### 2.5 Bad-pixel handling **before** demosaic

Hot pixels (thermally stuck-high), cold/dead pixels, and amp glow are
**single-photosite** defects. If you demosaic first, a single hot photosite is spread
by the interpolation kernel into a colored blob 2-4 px across, and is no longer
removable by a single-pixel median. Therefore defect correction belongs on the
**raw CFA mosaic**, before demosaic, using **same-color** neighbors (you may only
median a red defect against other red photosites — mixing colors creates a color
artifact). dcraw/libraw expose `bad_pixels` (a `.badpixels` map keyed by timestamp)
and `zero_is_bad`; astro pipelines instead derive a **defect map from the master dark**
(pixels whose dark signal is a MAD-outlier) and correct lights against it.

This is precisely why lumos has a separate `load_raw_cfa` path
(`src/raw/mod.rs:792-817`) that returns the **un-demosaiced** single-channel
`CfaImage`: calibration (dark/bias subtract → flat divide → defect correction) all
happens on the mosaic, and `CfaImage::demosaic` runs **last** (`cfa.rs:91-138`). This
ordering is the single most important structural decision in OSC astro decoding.

---

## 3. Demosaicing

When a sensor has a CFA, two-thirds of the color information at every pixel is
missing and must be reconstructed. The algorithm choice trades reconstructed
resolution against three artifact classes:

- **Zipper** — alternating light/dark along high-contrast edges, from interpolating
  across an edge in the wrong direction.
- **Maze / labyrinth** — a worm-like texture in fine detail, from inconsistent
  per-pixel direction decisions.
- **False color (chroma) fringing** — colored speckle where luminance detail
  exceeds the chroma sampling rate; especially bad on aliasing-prone (AA-filterless)
  sensors and on X-Trans.

### 3.1 Algorithm landscape (Bayer)

Quality/speed/artifact summary, cross-checked against RawPedia
(<https://rawpedia.rawtherapee.com/Demosaicing>), the librtprocess/RawTherapee
sources, and darktable's manual:

| Algorithm | Speed | Quality | Artifacts | Notes |
|-----------|-------|---------|-----------|-------|
| **Bilinear** | fastest | poor | heavy zipper, false color, blur | intra-channel; only for previews |
| **VNG4** (Variable Number of Gradients) | slow | low detail, very stable | strong maze suppression, loses detail | obsolete for general use; niche for lens-crosstalk green imbalance |
| **AHD** (Adaptive Homogeneity-Directed) | slow | dated | OK | "old, generally slow and inferior" (RawPedia) |
| **DCB** | medium | good | best false-color suppression on AA-less sensors | strong where false color dominates |
| **LMMSE** | slow | good on noisy data | suppresses maze on high-ISO | recommended for very noisy frames |
| **IGV** | slow | good on noisy data | strong moiré/maze suppression | recommended for noisy frames |
| **AMaZE** (Aliasing Minimization & Zipper Elimination) | slow | **state of the art** detail | minimal zipper; can color-overshoot on low-contrast/noisy areas | RawTherapee's historical default; best on clean low-ISO data |
| **RCD** (Ratio-Corrected Demosaicing) | fast-medium | near-AMaZE detail | **excellent on round edges / stars**, less overshoot than AMaZE | RawTherapee's current default |

The two state-of-the-art interior algorithms are **AMaZE** and **RCD**. RawPedia's
own wording: AMaZE "yields the best results in most cases" but "is also more prone to
color overshoots than RCD," while RCD "does an excellent job for round edges, for
example **stars in astrophotography**, while preserving almost the same level of
detail as AMaZE." That last sentence is why RCD is the natural Bayer choice for an
astro pipeline and why lumos implements RCD as its primary Bayer path.

**How RCD works** (`.tmp/refs/librtprocess/src/demosaic/rcd.cc`, originally
LuisSR/RCD-Demosaicing, MIT, by Luis Sanz Rodríguez): it (1) computes a directional
discrimination statistic from a vertical/horizontal high-pass filter squared
(`VH_Dir = V_stat/(V_stat+H_stat)`, `rcd.cc:128-160`) that is *invariant to chromatic
aberration*; (2) builds a low-pass filter combining R/G/B local samples
(`rcd.cc:162-169`); (3) reconstructs green at R/B sites using cardinal gradients and a
**ratio correction in the low-pass domain** rather than the usual color-difference
domain — the published green estimate is
`G_est = G·(1 + (LPF₀−LPF₂)/(LPF₀+LPF₂))` — which is exactly what reduces the pixel
*overshoot* that causes color speckle around stars; (4) reconstructs R/B from the now
complete green plus local color differences in the diagonal P/Q directions. The
ratio-in-LPF-domain step is the algorithm's defining idea. lumos's
`src/raw/demosaic/bayer/rcd.rs` is a SIMD port of this v2.3 code.

### 3.2 X-Trans: Markesteijn

Fuji X-Trans uses a 6×6 CFA (not 2×2), engineered to suppress moiré without an AA
filter — at the cost of making chroma reconstruction much harder. The reference
algorithm is **Markesteijn** (Frank Markesteijn, integrated into dcraw/libraw and
RawTherapee `xtrans_demosaic.cc`):

- **1-pass** — builds green from directional gradients over the 6×6 neighborhood,
  then R/B; fast, "fairly good results" (RawPedia/darktable). lumos implements
  Markesteijn **1-pass** (`src/raw/demosaic/xtrans/markesteijn.rs` +
  `markesteijn_steps.rs` + precomputed `hex_lookup.rs`).
- **3-pass** — iterates the direction selection three times, "leads to sharper
  results though you can only see this on low-ISO photos" (RawPedia); slower.

For X-Trans, false color in high-spatial-frequency luminance is the dominant artifact;
both Markesteijn variants apply careful direction selection to limit it. On *noisy*
astro subs the 1-pass/3-pass quality gap largely vanishes (the noise dominates), so
1-pass is the pragmatic astro choice — which is what lumos picks.

### 3.3 Astro-specific alternatives — and why chroma processing is dangerous

Terrestrial demosaic pipelines routinely follow the interpolation with **chroma
denoising** and **false-color suppression** (median-filtering the color-difference
planes, "defringe," etc.). For astrophotography these steps are actively harmful:

- They are **nonlinear, spatially-varying** operations. A chroma median replaces a
  pixel's color with a neighborhood statistic, which **moves flux between pixels** and
  destroys the per-pixel photometric value. A red star on a black background can be
  desaturated toward the background by false-color suppression.
- They assume natural-image chroma is smooth — but a star field is the opposite:
  sparse, high-contrast point sources on a dark background, exactly the signal these
  filters treat as "noise to remove."

So the astro rule is: demosaic with a **linear-as-possible, detail-preserving**
algorithm (RCD/AMaZE/Markesteijn), and **skip all chroma smoothing and false-color
suppression**. Accept a little residual false color around the brightest stars
rather than corrupt photometry everywhere.

Three structural alternatives sidestep demosaic artifacts entirely:

1. **Bayer / CFA drizzle** (Fruchter & Hook drizzle applied to the *mosaic*). Each
   raw photosite is dropped into the output channel its filter color dictates;
   sub-pixel dithering between many frames fills the gaps. This *never interpolates
   color* and so "avoids the artefacts that occur with all debayering algorithms,"
   with better noise and a corrected green bias (Siril docs
   <https://siril.readthedocs.io/en/latest/preprocessing/drizzle.html>; PixInsight
   forum). Requirements: **the CFA pattern must be preserved through stacking**
   (do *not* debayer before drizzle), and frames must be **dithered**. Recommended
   parameters for a normally-sampled OSC sensor: **scale = 1.0, pixfrac = 1.0** —
   upsampling (scale > 1) needs exponentially more subs because R/B sites are sparse.
   lumos already has a drizzle stage (`src/drizzle/`); feeding it CFA data is the
   natural astro path to a demosaic-free OSC pipeline.
2. **Superpixel** (a.k.a. half-resolution / `bin 2×2`): collapse each 2×2 Bayer cell
   to one output RGB pixel (R, average-of-2-G, B). Halves resolution but is
   *exactly* linear, artifact-free, and trivially correct photometrically — ideal for
   well-sampled or short-focal-length OSC data, or as a fast, trustworthy preview.
3. **Split-CFA / channel extraction**: treat each color's photosites as three
   independent sub-images (each at half resolution, on its own grid) and process them
   like three mono channels. Useful when you want fully independent per-channel
   registration/stacking.

### 3.4 Demosaic-first-vs-denoise-first

The literature is genuinely split on whether to denoise before or after demosaicing
("A Review of an Old Dilemma," <https://arxiv.org/pdf/2004.11577>). For *astro* the
question is largely moot because **temporal stacking** is the denoiser of choice: you
average/reject many calibrated subs, which suppresses noise without any spatial
filter touching the photometry. The only spatial step that belongs *before* demosaic
is single-pixel **defect correction** (§2.5), which is not denoising.

---

## 4. Recommended best-practice implementation

A concrete, opinionated Stage-1 recipe for an OSC astro pipeline.

**FITS path:**

1. Open via cfitsio/rust-fitsio; never parse the array yourself.
2. Read into the widest integer type that fits `BITPIX`, letting cfitsio apply
   `BZERO`/`BSCALE` and recognize the unsigned-16/32 conventions. Promote to `f32`
   (or `f64` for `BITPIX = 64` / large physical ranges) after scaling.
3. Mask undefined pixels: integer `BLANK` against the *raw* value; NaN/±Inf for
   float. Carry a mask rather than substituting 0 if downstream stacking can consume
   masks (substituting 0 biases means).
4. For integer data, normalize by the BITPIX max (exact). For float data with a
   *known* source range, divide by that constant. For *unknown* float range, the
   max-heuristic is acceptable but record that the scale is per-file and ensure the
   stacking stage re-normalizes inter-frame.
5. Apply orientation/CFA corrections from `ROWORDER`, `XBAYROFF`, `YBAYROFF`,
   `BAYERPAT` *before* using the pattern. Keep OSC FITS as a mosaic if you intend to
   calibrate-then-debayer or CFA-drizzle.

**RAW path:**

1. libraw `open_buffer` → `unpack`. Read `cblack[]`, `black`, `maximum`, `cam_mul`,
   `filters`, margins.
2. Consolidate black exactly per `adjust_bl()` into `common` + per-channel deltas
   (lumos's `consolidate_black_levels` is a correct reference).
3. Subtract black in linear ADU. For **calibration frames, do not clamp to zero** —
   keep sub-pedestal scatter so master darks/bias are unbiased. Clamp only the white
   end, and only as a saturation flag.
4. White balance: either leave unity (preferred for rigorous color calibration later)
   or apply camera multipliers **min-normalized to 1.0** (libraw `!highlight`
   behavior). Either way it is a per-channel multiply — keep it linear. Never apply
   `output_color`, gamma, or auto-bright.
5. **Calibrate on the mosaic**: dark/bias subtract, flat divide with **per-CFA-color
   means** (lumos `divide_by_normalized_cfa`), then defect-correct against a
   dark-derived map using same-color neighbors.
6. Demosaic **last**, with **RCD** (Bayer) or **Markesteijn 1-pass** (X-Trans), and
   **no chroma denoise / no false-color suppression**. Offer **superpixel** and
   **CFA-drizzle** as artifact-free alternatives, defaulting CFA-drizzle to
   scale = pixfrac = 1.0 and requiring dithered, undebayered input.

**Numerical considerations:**

- Do black subtraction and normalization in one fused pass when possible (lumos does)
  but keep the **common-vs-delta decomposition** so the bulk subtract is SIMD-uniform.
- Accumulate flat means in `f64` (lumos does) — summing millions of `f32` in `f32`
  loses precision and shifts the normalization.
- Demosaic interior math in `f32` is fine; RCD uses `eps = 1e-5`, `epssq = 1e-10`
  guards against divide-by-zero in its ratio/gradient terms (`rcd.cc:72-73`) — port
  those guards, don't drop them.
- Keep f64 for geometric/WCS coordinate math (RA in HMS×15, etc.).

---

## 5. Pitfalls & anti-patterns (practices to avoid)

- **Demosaicing before calibration.** The cardinal sin. Spreads hot pixels into
  colored blobs, breaks the per-photosite dark/bias model, and makes per-channel flat
  division wrong. Always calibrate the mosaic, debayer last.
- **Ignoring `BZERO`/`BSCALE` (especially the unsigned-16 shift).** Reads a `uint16`
  light as signed-centered; half the pixels go negative. Always go through cfitsio.
- **Applying a nonlinear WB / gamma / tone curve / auto-brightness at decode.**
  Destroys `pixel ∝ photons`; flats, stacking statistics, and photometry all break.
  libraw's `output_color`, nonzero `gamm`, and auto-bright must be off.
- **Highlight recovery / clipped-channel reconstruction.** Invents non-physical data
  in saturated stars. Detect-and-mask saturation instead.
- **Double black subtraction.** Subtracting libraw's black *and then* a master-bias
  that already includes the pedestal removes it twice and pushes the background
  negative. Know whether your master frames are pre- or post-pedestal.
- **Clamping calibration frames to ≥ 0.** Read noise is symmetric; clamping
  sub-pedestal scatter to zero injects a positive bias into master darks/bias. (lumos
  clamps at load; it mitigates by keeping negatives during `CfaImage::subtract`, but
  the load-time clamp on the *raw* frames is still a latent bias source — see §6.)
- **Single-scalar black on a per-channel sensor.** Leaves an additive color cast flats
  can't remove. Use the full `adjust_bl()` per-channel model.
- **Single global flat mean on a non-white flat (LED/twilight).** Shifts color. Use
  per-CFA-color means (lumos's `divide_by_normalized_cfa` does this correctly).
- **Chroma denoising / false-color suppression on astro data.** Nonlinear,
  flux-moving, treats stars as noise. Skip entirely; rely on temporal stacking.
- **sRGB / ICC color management on linear astro data.** Linear sky data is not an sRGB
  image; applying a display transform at decode is meaningless and nonlinear.
- **Ignoring `ROWORDER`/`XBAYROFF`/`YBAYROFF`.** Produces swapped R/B or a color maze
  on OSC FITS.
- **Computing statistics over the masked sensor border.** Optical-black pixels are not
  sky; crop to the active rectangle first.
- **Treating float-FITS BLANK via `BLANK` keyword, or integer-FITS NaN.** `BLANK` is
  integer-only; NaN is the float blank. Test the correct one against the correct value.

---

## 6. How lumos currently does it — and gaps/opportunities

**FITS** (`src/astro_image/fits.rs`): solid. Goes through rust-fitsio/cfitsio so
`BZERO`/`BSCALE` and endianness are handled; preserves signed-vs-unsigned BITPIX for
correct normalization (`image_type_to_bitpix`); handles 2-D/3-D axis order and planar
RGB correctly; composes `ROWORDER` + `XBAYROFF` + `YBAYROFF` + `BAYERPAT` for OSC FITS;
parses RA/Dec in all three common encodings; sanitizes float NaN/Inf to 0.

**RAW** (`src/raw/mod.rs`): strong. `consolidate_black_levels` is a faithful port of
libraw `adjust_bl()` including the 2×2/X-Trans spatial folding, common-minimum
extraction, and per-channel deltas. WB via `compute_wb_multipliers` matches libraw's
`!highlight` min-normalization and is applied only on the light path, never on
calibration. Sensor dispatch picks our fast RCD (Bayer) / Markesteijn-1-pass
(X-Trans) and falls back to libraw configured for **linear** output
(`gamm={1,1}`, `output_color=0`, `no_auto_bright`). The separate `load_raw_cfa` path
returning an un-demosaiced `CfaImage` is exactly right and enables the
calibrate-then-debayer ordering, with flat division using per-CFA-color means.

**Gaps / opportunities:**

1. **Load-time zero-clamp biases calibration frames.** `normalize_u16_to_f32_parallel`
   and `apply_channel_corrections` clamp to `[0, 1]` (`mod.rs:259`, `normalize.rs`).
   For *light* frames this matches libraw. For *dark/bias* frames the lower clamp
   discards symmetric read-noise scatter below the pedestal and biases the master
   high. `CfaImage::subtract` keeps negatives, but the raw frames are already clamped
   before they are stacked into a master. Consider an unclamped calibration load path
   (subtract pedestal, allow negative f32).
2. **Integer-FITS `BLANK` is not handled.** `normalize_fits_pixels` only sanitizes
   NaN/Inf, and only for float BITPIX (`fits.rs:146-163`). Integer FITS carrying a
   `BLANK` value will pass through as ordinary finite data. Read `BLANK` and mask it
   (against the pre-scale value) for integer images.
3. **Float-FITS divide-by-max is per-file and lossy.** The 2.0 threshold + divide-by-max
   heuristic (`fits.rs:146-163`) can't distinguish `[0,1.5]` HDR from small physical
   values and gives sibling frames different scales. It works because the stacking
   stage re-normalizes, but a known-range override (or carrying the source range in
   metadata) would be safer for photometry.
4. **No native CFA-drizzle wiring.** lumos has both a drizzle stage and an
   un-demosaiced `CfaImage`, but they aren't connected into a CFA/Bayer-drizzle path.
   Given RawPedia/Siril guidance that CFA drizzle is the artifact-free OSC ideal, a
   `drizzle_stack` variant that consumes mosaics (scale = pixfrac = 1.0, dithered) is
   a high-value addition.
5. **No superpixel / split-CFA mode.** A trivial, exactly-linear half-res debayer
   would be a useful fast/trustworthy alternative and a photometric ground truth for
   testing the RCD path.
6. **Defect correction is dark-derived (good) but lives in the calibration module**,
   not the load module — fine architecturally, but the load docs should make the
   "defects corrected on the mosaic before demosaic" ordering explicit so callers
   don't accidentally demosaic the uncalibrated `load_raw` output for OSC data.

---

## 7. References

### Source code (under `.tmp/refs/`)

- `LibRaw/src/utils/utils_libraw.cpp:464-545` — `adjust_bl()`: the canonical
  per-channel + spatial black-level consolidation lumos ports.
- `LibRaw/src/preprocessing/subtract_black.cpp:24-91` — `subtract_black_internal()`:
  the actual `CLIP(val − cblack[c])` subtraction and channel-max update; shows
  libraw's default zero-clamp.
- `LibRaw/src/postprocessing/postprocessing_utils_dcrdefs.cpp:106-249` —
  `scale_colors()`: WB multiplier computation and the `!highlight ⇒ dmax=dmin`
  min-normalization that lumos mirrors.
- `LibRaw/src/postprocessing/dcraw_process.cpp` — demosaic dispatch by quality level
  (`n=0` linear … `n=3` AHD, `n=11/12` DHT/AAHD; X-Trans 1-pass/3-pass).
- `librtprocess/src/demosaic/rcd.cc` — RCD v2.3 reference (LuisSR, MIT): direction
  statistic, low-pass filter, ratio-corrected green, eps guards.
- `librtprocess/src/demosaic/markesteijn.cc` — X-Trans Markesteijn reference.
- `RawTherapee/rtengine/{rcd_demosaic,amaze_demosaic_RT,dcb_demosaic,lmmse_demosaic,
  igv_demosaic,vng4_demosaic_RT,ahd_demosaic_RT,xtrans_demosaic}.cc` — the full
  Bayer/X-Trans demosaic family for cross-comparison.
- `cfitsio/` — FITS I/O: `BZERO`/`BSCALE` scaling, integer↔float, `BLANK`/null,
  big-endian byte order, unsigned conventions.
- `rust-fitsio/` — the Rust cfitsio binding lumos actually links.
- `astropy/io/fits/` — FITS conventions: unsigned-16 (`BZERO=32768`), scaling,
  NaN/BLANK, header keyword aliases, `ROWORDER`/`BAYERPAT`.

### Literature / online

- FITS Standard 4.0 (HEASARC dictionary) — <https://heasarc.gsfc.nasa.gov/docs/fcg/standard_dict.html>
  — `physical = BZERO + BSCALE × array`; `BLANK` integer-only; `BSCALE/BZERO`
  discouraged for floats; big-endian arrays.
- FITS floating-point agreement — <https://fits.gsfc.nasa.gov/fp89.txt> — NaN as the
  float blank; ignore `BLANK` for `BITPIX < 0`.
- astropy image-data docs — <https://docs.astropy.org/en/stable/io/fits/usage/image.html>
  — int16 + `BZERO=32768`/`BSCALE=1` is auto-interpreted as uint16.
- MaxIm DL FITS header definitions — <https://cdn.diffractionlimited.com/help/maximdl/FITS_File_Header_Definitions.htm>
  — `BAYERPAT`, `XBAYROFF`/`YBAYROFF`, temperature/gain keyword forms.
- RawPedia "Demosaicing" — <https://rawpedia.rawtherapee.com/Demosaicing> — algorithm
  quality/artifact comparison; AMaZE default, RCD best for round edges/stars,
  Markesteijn 1-pass vs 3-pass.
- LuisSR RCD-Demosaicing — <https://github.com/LuisSR/RCD-Demosaicing> — RCD origin,
  ratio-in-LPF-domain green estimate, CA-invariant direction statistic.
- Siril drizzle docs — <https://siril.readthedocs.io/en/latest/preprocessing/drizzle.html>
  — CFA/Bayer drizzle for OSC: scale = pixfrac = 1.0, preserve CFA, no debayer before
  drizzle, avoids debayering artifacts.
- PixInsight forum, "Bayer drizzle instead of de-Bayering" —
  <https://pixinsight.com/forum/index.php?threads/bayer-drizzle-instead-of-de-bayering-with-osc.12996/>
  — practical CFA-drizzle rationale and green-bias correction.
- darktable demosaic manual — <https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/demosaic/>
  — RCD/AMaZE/VNG/Markesteijn behavior and defaults.
- astropy CCD Data Reduction Guide / ccdproc — <https://www.astropy.org/ccd-reduction-and-photometry-guide/>
  and <https://ccdproc.readthedocs.io/en/latest/reduction_toolbox.html> — canonical
  calibration order: overscan → trim → bias → dark (scaled) → flat; debayer separate.
- "A Review of an Old Dilemma: Demosaicking First, or Denoising First?" —
  <https://arxiv.org/pdf/2004.11577> — the unresolved order debate (moot for astro,
  where temporal stacking is the denoiser).
- Clark, "Astrophotography Image Processing Using Modern Raw…" —
  <https://clarkvision.com/articles/astrophotography.image.processing/> — keeping the
  raw pipeline linear and the DN→flux ordering.
