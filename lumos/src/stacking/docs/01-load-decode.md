# Stage 1 — Load & Decode: Best Practices & Algorithms

Reference material for the `lumos` astro-image library. This document covers the
first stage of an astrophotography pipeline: reading FITS files and camera RAW
files, decoding sensor data, and (when applicable) demosaicing a color-filter-array
sensor — producing a **linear, planar, floating-point** image that downstream
calibration, registration, stacking, and photometry can trust.

Claims here are cross-checked against the FITS Standard 4.0, the cloned upstream
sources under `.tmp/refs/` (LibRaw, librtprocess, RawTherapee, cfitsio, astropy),
and the published astrophotography/demosaicing literature. Where authoritative
sources disagree, that is called out explicitly. **Pass 2** additionally parsed the
primary-source PDFs (FITS Standard 4.0, the IPOL Malvar reproduction, Siril drizzle
docs) and re-read the RCD/Markesteijn/`adjust_bl` source line-by-line to verify the
formulas; corrected claims are flagged with "**Correction (pass 2):**" and the parsed
PDFs are listed under References. **Pass 3** manually re-verified every formula and
source line-reference against the cloned code and parsed papers (the FITS scaling/
BLANK/endianness quotes, `adjust_bl`/`scale_colors`/`subtract_black`, the RCD and
Markesteijn math and lumos's ports, and the Malvar gains/kernel/PSNR), re-checked the
online claims against RawPedia/astropy/darktable, corrected the RawTherapee-vs-darktable
default-demosaic attribution (flagged "**Correction (pass 3):**"), and added §2.6 on
standard (TIFF/PNG/JPEG) inputs.

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
   (`imaginarium::Buffer2<f32>` in lumos), which is the natural layout for per-channel
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

`BITPIX` declares the on-disk sample type. The Standard fixes the integer sizes
(§5.2): 8-bit are **unsigned** binary integers (0–255); 16/32/64-bit are **two's
complement signed** binary integers (§5.2.2–5.2.4, e.g. 16-bit range −32768…+32767);
`-32`/`-64` are IEEE-754 single/double float (§5.3). The physical value is
reconstructed by the FITS Standard 4.0 scaling equation (§4.4.2, Eq. 3, verified pass 2
from the parsed standard `.tmp/papers/fits40.txt:2224`):

```
physical value = BZERO + BSCALE × array value
```

with defaults `BZERO = 0.0`, `BSCALE = 1.0`. The Standard's exact wording: BSCALE is
"the coefficient of the linear term in the scaling equation, the ratio of physical value
to array value at zero offset"; BZERO is "the physical value corresponding to an array
value of zero."

**The unsigned convention is the critical landmine — and it is the Standard, not a
hack.** §5.2.5 states plainly that "the FITS format does not support a native unsigned
integer data type (except for the unsigned 8-bit byte data type) therefore unsigned
16-bit, 32-bit, or 64-bit binary integers cannot be stored directly… the appropriate
offset must be applied… which is then stored in the FITS file. The BZERO keyword shall
record the amount of the offset needed to restore the original unsigned value." Table 11
gives the offsets: `uint16` ⇒ `BITPIX=16, BZERO=32768`; `uint32` ⇒ `BZERO=2147483648`;
`uint64` ⇒ `BZERO=9223372036854775808`; and the converse for *signed* 8-bit
(`BITPIX=8, BZERO=−128`). In all cases `BSCALE` keeps its default 1.0. So a `uint16`
value of 0 is stored on disk as the signed value −32768. A footnote (§4.4.2.5 n.9) notes
the offset is most efficiently applied by simply flipping the most-significant bit. Any
loader that ignores `BZERO` reads a `uint16` light as a signed array centered on zero —
half the pixels go negative and the histogram is destroyed. astropy applies the
convention by default; cross-check <https://docs.astropy.org/en/stable/io/fits/usage/image.html>.

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

For **integer** images, undefined pixels are flagged by the `BLANK` keyword. §4.4.2.5
(verified pass 2, `.tmp/papers/fits40.txt:2262`): "This keyword shall be used only in
headers with positive values of BITPIX (i.e., in arrays with integer data)… an integer
that specifies the value that is used within the integer array to represent pixels that
have an undefined physical value." The Standard nails the raw-vs-scaled subtlety
explicitly: **"If the BSCALE and BZERO keywords do not have the default values… then the
value of the BLANK keyword must equal the actual value in the FITS data array that is
used to represent an undefined pixel and not the corresponding physical value (computed
from Eq. 3)."** Worked example given verbatim in the Standard: for `uint16`
(`BZERO=32768, BSCALE=1`), to make the *physical* value 0 mark an undefined pixel, set
`BLANK = −32768`, because −32768 is the actual stored array value.

For **float** images, §5.3 is equally explicit: "The BLANK keyword should not be used
when BITPIX = −32 or −64; rather, the IEEE NaN should be used to represent an undefined
value. Use of the BSCALE and BZERO keywords is not recommended." The full IEEE set
(±Inf, denormals, NaN) is permitted in FITS interchange (§5.3), but only **NaN** is the
sanctioned blank.

A correct reader therefore:

- integer image: compare each *raw array value* against `BLANK` (before scaling), and
  mask matches — never compare the scaled physical value, which may not be exactly
  representable;
- float image: treat NaN (and, defensively, ±Inf — which the standard does not
  define as data) as undefined.

cfitsio handles the raw-value comparison internally when you set a null value via
`fits_read_imgnull` / `fits_set_imgnull`. If you read scaled floats and then test for
NaN (lumos's approach, `fits.rs:151-158`), you correctly catch float blanks but
**silently miss integer `BLANK` pixels**, because the integer `BLANK` value scales
to an ordinary finite float. This is a real gap for data from instruments that
write integer `BLANK` (uncommon in amateur OSC/CMOS data, common in survey/archival
integer FITS).

### 1.3 Endianness

FITS arrays are **big-endian** by definition. §3.3.2 (verified pass 2,
`.tmp/papers/fits40.txt:1075`): "The individual data values shall be stored in
big-endian byte order such that the byte containing the most-significant bits of the
value appears first in the FITS file, followed by the remaining bytes, if any, in
decreasing order of significance." This applies to integers (two's-complement, MSB
first) and to IEEE floats alike. Byte swapping on little-endian hosts (x86, ARM) is
mandatory and is handled inside cfitsio; never read FITS bytes raw. This is a
non-issue if you go through cfitsio/rust-fitsio and a guaranteed corruption bug if
you don't.

Array storage order (§3.3.2): Axis 1 (`NAXIS1`) varies fastest, then Axis 2, etc. —
"the same order as in multi-dimensional arrays in the Fortran programming language."
That is why C/row-major bindings return the shape reversed (see §1.5).

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

### 1.8 Multi-extension and compressed FITS (the load gap lumos hasn't closed)

A FITS file is one or more HDUs (Header-Data Units): a mandatory **primary HDU**
followed by optional **extensions** (`XTENSION` = `IMAGE`, `TABLE`, `BINTABLE`,
§3.4.1). A file with extensions is a **multi-extension FITS (MEF)**. Real instrument
and survey data routinely put the science image in an `IMAGE` extension (the primary
HDU may be empty, `NAXIS = 0`), and place masks / variance / WCS in further extensions.

**Tiled image compression** (FITS Standard §10, verified pass 2,
`.tmp/papers/fits40.txt:5913`) is the bigger trap, because the result *looks like a
table*. The image is cut into a "rectangular grid of subimages or tiles" (default tiling
= one row per tile, `ZTILE1 = NAXIS1`), each tile is compressed and stored as one row of
a variable-length column in a **`BINTABLE`** extension, with `ZIMAGE = T` marking "this
extension should be interpreted as an image rather than a table." The original geometry
is carried in mandatory keywords: `ZCMPTYPE` (algorithm), `ZBITPIX` (= original
`BITPIX`), `ZNAXIS`, `ZNAXISn`. Supported algorithms (§10.4): **`RICE_1`** (Rice — the
common, fast astronomy default), **`GZIP_1`/`GZIP_2`**, **`PLIO_1`** (IRAF mask
run-length), **`HCOMPRESS_1`**. fpack/funpack are the reference tools.

Float images get a further subtlety (§10.2): lossless float compresses poorly, so the
floats are often **quantized to scaled integers** per tile via `I = round((F − ZZERO) /
ZSCALE)`, with per-tile `ZSCALE`/`ZZERO` columns. `ZSCALE` is set to a fraction `Q` of
the background RMS noise (≈ `log2(Q) + 1.792` bits of noise preserved per pixel). To
avoid a **systematic photometric bias** in faint regions (coarse quantization pulls the
sky background toward the nearest level), the Standard mandates **subtractive dithering**:
`I = round((F − ZZERO)/ZSCALE + R_i − 0.5)`, restored with the same `R_i` per pixel;
`ZQUANTIZ` records `NO_DITHER` / `SUBTRACTIVE_DITHER_1/2` and `ZDITHER0` the RNG seed.
This is directly astro-relevant: a naive float-FITS reader that decompresses without
honoring `ZSCALE`/`ZZERO`/`ZQUANTIZ` reads garbage; one that ignores the dither
restoration reintroduces the faint-end bias the dither was designed to remove.
Undefined pixels survive compression via `ZBLANK` (recommended −2147483648) for floats,
or the ordinary integer `BLANK` for integer data (§10.2.2).

**lumos gap:** `load_fits` opens only the **primary HDU** (`fits.rs:21`,
`fptr.primary_hdu()`) and errors if it is a table. It therefore cannot read (a) an MEF
where the image is in an `IMAGE` extension, nor (b) any tile-compressed FITS (the image
lives in a `BINTABLE` with `ZIMAGE=T`). cfitsio *can* transparently decompress these
(its "compressed image" API treats a `.fz` HDU as a normal image), and rust-fitsio
exposes HDU iteration — so the fix is to iterate HDUs, pick the first with image data
(or the one with `ZIMAGE=T`), and let cfitsio handle decompression. This is a real-world
hole for anyone feeding lumos `fpack`-compressed survey/archive frames or DSLR-to-FITS
exports that wrap the image in an extension.

### 1.9 WCS keyword preservation through load

Plate-solved frames carry a **World Coordinate System** describing pixel→sky mapping
(§8). The conversion pipeline is a fixed chain (§8, Fig. 2): pixel coords →
(`CRPIXj`, linear `PCi_j` or `CDi_j`) → intermediate pixel coords → (`CDELTi`) →
intermediate world coords → (`CTYPEi`, `CRVALi`, projection params `PVi_m`) → world
coords. For tile-compressed images the Standard *strongly recommends* copying all
original-image keywords verbatim into the table header, "even in cases where the keyword
is not normally expected to occur in the header of a binary-table extension (e.g., the
BSCALE and BZERO keywords, or the world-coordinate-system keywords such as CTYPEn,
CRPIXn, and CRVALn)" (§10.1.2). A loader that demosaics/registers and then re-saves
must propagate the WCS (and update `CRPIXj` if it crops/bins), or downstream astrometric
matching is lost. lumos currently parses only the reference point (`CRVAL1/2` as an
RA/Dec fallback, `fits.rs:209`) and does not retain the full WCS through load — fine for
a star-pattern registration pipeline that re-derives geometry, but it means lumos cannot
*preserve* an existing plate solution.

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
(`.tmp/refs/LibRaw/src/utils/utils_libraw.cpp:464-545`). The data structures (from
`libraw/libraw_types.h:655-660`, verified pass 2; `cblack` is `unsigned[4104]`,
`LIBRAW_CBLACK_SIZE = 4104`):

- `C.black` — a scalar global pedestal (`unsigned black`).
- `C.cblack[0..3]` — per-channel pedestals for the 2×2 CFA positions. The mapping is by
  the `FC(row,col)` color macro: `cblack[FC(0,0)], cblack[FC(0,1)], cblack[FC(1,0)],
  cblack[FC(1,1)]`, with the *second green* remapped to channel index 3 (so the layout is
  effectively R, G1, B, G2).
- `C.cblack[4]`, `C.cblack[5]` — the *dimensions* of an *additional* spatial black
  pattern (rows × cols, e.g. a small repeating offset grid), with `cblack[6 + …]` holding
  that pattern in row-major order.
- `C.maximum` — the white level (saturation ADU); `C.data_maximum` — the brightest value
  actually present; `C.dmaxall` / per-channel maxima updated during subtraction.

A parallel **DNG** set exists — `dng_black`, `dng_cblack[4104]`, plus *floating-point*
`dng_fblack` / `dng_fcblack[4104]` (`libraw_types.h:237-240`) — because the DNG spec
carries black levels (and `BlackLevelDeltaH/V`) as rationals that need not be integers.
LibRaw folds these into the same `black`/`cblack` model before `adjust_bl()`.

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

lumos's port is structurally faithful but splits direct lights from calibration inputs:

- *Direct lights*: a SIMD pass removes the
  `common` pedestal uniformly (`normalize_u16_to_f32_parallel`, which also does the
  `1/(max−common)` scale in the same pass), then a second pass that applies the
  per-channel delta (`apply_bayer_black_deltas`). This is
  exactly libraw's "common + delta" decomposition, just executed in two vectorizable
  passes, with the final result clamped to `[0, 1]`.
- *Calibration inputs*: `normalize_active_area::<false>` performs the same common and
  per-channel subtraction without a floor or ceiling. Dark and bias samples below the
  pedestal therefore remain negative instead of biasing their stacked masters upward.

**Why per-channel matters:** if you subtract a single scalar black from a sensor
whose green channels sit 8 ADU higher than red, the residual offset survives into the
demosaic and shows up as a faint color cast that flat-fielding cannot remove (flats
correct *multiplicative* response, not *additive* offset). Photometry on such data
has a per-color zero-point error.

### 2.3 White balance / camera multipliers — why lumos keeps unity

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

lumos deliberately keeps camera white balance at unity on every RAW path, including
the LibRaw fallback. Direct demosaic and calibrate-then-demosaic therefore produce the
same raw-linear color domain: each channel stays proportional to its sensor signal,
which is the correct input for per-channel flat division, noise modeling, stacking,
and photometry. The camera multipliers remain available as canonical
`AstroImageMetadata::camera_white_balance` values in `[R, G1, B, G2]` order; they are
metadata only. A later explicit color-calibration or display operation may apply them
without changing the science pipeline's decode contract.

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

**Saturation is per-channel and interacts with demosaic.** The true saturation
threshold is at the *raw* white level `maximum`, **per CFA color**. Unity white balance
keeps that threshold in the decoded raw-linear domain, but demosaic still spreads
clipping: once a saturated photosite is interpolated, its clipped
  value bleeds into neighbors, so the saturated region after demosaic is larger and
  fuzzier than the true clipped set. Hence saturation masks, like defect maps, are most
  reliable when computed on the **mosaic** (§2.5) and carried through, rather than
  re-derived from the demosaiced image.

**Linearity below saturation.** Modern CMOS is linear to well within a percent over most
of its range but can roll off near full well; some sensors also have a small nonlinear
toe near black. LibRaw/DNG model this with a linearization table (`LinearizationTable`
in DNG, applied at unpack) — if present it must be applied *before* black subtraction to
restore proportionality. lumos assumes post-unpack linearity (no extra linearization
table handling) and relies on the camera's native linearity, which is correct for the
mainstream astro CMOS it targets but would mis-handle a sensor that ships a DNG
linearization curve.

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

### 2.6 Standard formats (TIFF/PNG/JPEG) — the linearity trap

FITS and RAW are not the only inputs. `AstroImage::from_file` (`src/astro_image/mod.rs:276-287`)
routes a third class — `tiff`/`tif`/`png`/`jpg`/`jpeg` — to the imaginarium loader
(`Image::read_file(path)?.into()`), with **no gamma decode and no linearization** applied.
That is a quiet correctness hazard, because these formats arrive in two photometrically
incompatible states and nothing in the file forces the right interpretation:

- **8-bit PNG/JPEG are almost always sRGB-gamma-encoded and already demosaiced** (and JPEG
  is lossy, 4:2:0-chroma-subsampled, and block-quantized). Loaded verbatim, the pixels are
  a nonlinear function of photons — every premise of Stage 1 (`pixel ∝ photons`) is broken,
  so flat division, stacking statistics, and photometry are invalid. There is no safe way
  to "rescue" them: the gamma is recoverable in principle (apply the inverse sRGB transfer
  function) but the 8-bit quantization and JPEG loss are not, and a file may instead be
  *already linear* 8-bit (rare) — undetectable from the pixels alone.
- **16-bit TIFF is ambiguous** in exactly the way headerless float FITS is (§1.4): capture
  tools (SharpCap, some DSLR tethering) can write **linear** 16-bit TIFF that is perfectly
  usable, while anything exported from an image editor is gamma-encoded and tone-mapped.
  TIFF *can* carry a `TransferFunction`/`ColorSpace` tag, but most astro tools don't, so a
  loader cannot reliably tell the two apart.

**Best practice:** treat 8-bit PNG/JPEG as **display artifacts, not science data** — accept
them only for previews/thumbnails or for already-processed "finished" images, never as
stacking input. Accept 16-bit TIFF only when the source is *known* linear, and record that
provenance out-of-band (same discipline as headerless float FITS). If non-linear input must
be ingested for a non-photometric purpose, apply the explicit inverse-sRGB transfer function
at load so at least the gamma is undone — but do not pretend the result is calibration-grade.

**lumos status:** the standard-format path performs neither linearization nor a linear/
nonlinear provenance check; it trusts the caller to feed it linear data. For a pipeline whose
whole contract is linearity this is worth at least a documented warning at the API boundary
(and ideally refusing 8-bit lossy formats as stacking input). See §6 gap 9.

---

## 3. Demosaicing

When a sensor has a CFA, two-thirds of the color information at every pixel is
missing and must be reconstructed. The Bayer mosaic samples green on a quincunx
(½ the pixels) and red/blue each on ¼; green therefore has twice the sampling rate
of chroma, and **every good demosaic algorithm exploits the fact that R/G/B
edges are correlated** — it estimates the high-frequency luminance detail from the
dense green channel and applies it to the sparse R/B channels (the "constant
color-difference" or "constant color-ratio" assumption). The algorithm choice trades
reconstructed resolution against four artifact classes:

#### Artifact taxonomy

- **Zipper** — alternating light/dark "teeth" running *along* a high-contrast edge,
  from interpolating *across* the edge in the wrong direction (mixing the bright and
  dark sides). Caused by direction-agnostic or wrongly-directed interpolation.
- **Maze / labyrinth** — a worm-like / fingerprint texture in fine, near-Nyquist
  detail, from *inconsistent neighbor-to-neighbor direction decisions* (one pixel votes
  horizontal, its neighbor vertical). It is the failure mode of directional methods
  whose direction estimate is noisy.
- **False color (chroma) fringing** — spurious colored speckle where luminance detail
  exceeds the chroma sampling rate: the under-sampled R/B channels alias, so a neutral
  fine texture acquires color. Worst on aliasing-prone (AA-filterless) sensors and on
  X-Trans; on astro data it appears as colored rims on bright stars and along sharp
  diffraction spikes.
- **Color moiré** — large-scale, low-frequency rainbow banding when a periodic
  high-frequency pattern (a regular texture, or a finely-sampled diffraction pattern)
  beats against the CFA period. The structured, large-scale cousin of false color.

#### Which algorithm suppresses which

| Artifact | Root cause | Suppressed by | Worsened by |
|----------|------------|---------------|-------------|
| Zipper | cross-edge interpolation | edge-directed methods (AHD, AMaZE, RCD, Markesteijn) | bilinear, fixed-direction |
| Maze | noisy/inconsistent direction votes | direction *smoothing* / homogeneity voting (AHD, Markesteijn homogeneity map; RCD's `VH_Disc` neighborhood refinement) | high-ISO noise feeding the direction estimator |
| False color | chroma aliasing | color-difference/ratio interpolation + (terrestrial) chroma median; AMaZE/RCD anti-overshoot | bilinear; any per-channel-independent method |
| Color moiré | periodic detail beating CFA | VNG/LMMSE/IGV averaging; AA filter in hardware | sharp AA-less sensors |

The cross-cutting astro caveat: the terrestrial cure for false color is **chroma
median / defringe**, which is exactly the step that destroys point-source photometry
(§3.3). So an astro pipeline must pick an algorithm whose *interpolation itself*
minimizes false color (RCD's anti-overshoot ratio estimate) rather than relying on a
post-hoc chroma filter.

### 3.0 Malvar–He–Cutler — the linear gradient-corrected baseline

Before the directional methods, the canonical *linear* algorithm is **Malvar, He &
Cutler 2004** (gradient-corrected linear interpolation). It is worth stating exactly
because it is the cheapest method that is still **strictly linear** — and a strictly
linear demosaic is photometrically the safest (it is a fixed convolution; flux is
conserved up to the kernel, no data-dependent decisions). Verified pass 2 from the
peer-reviewed IPOL reproduction (Getreuer 2011, `.tmp/papers/malvar_ipol.txt`):

Start from bilinear (green = mean of 4 axial neighbors; R/B = mean of 4 diagonal
neighbors), then add a **Laplacian cross-channel correction** (following Pei & Tam):

```
Ĝ(i,j) = Ĝ_bilinear(i,j) + α·ΔR(i,j)        at a red site
ΔR(i,j) = R(i,j) − ¼·(R(i−2,j) + R(i+2,j) + R(i,j−2) + R(i,j+2))   // 5-point Laplacian of R
```

i.e. the green estimate is nudged by the *second derivative of the known channel at the
same site* — encoding "R and G share high-frequency detail." Symmetric corrections fill
R at green sites (β·ΔG, a 9-point Laplacian), R at blue sites (γ·ΔB, 5-point), and the
blue analogues. The gains are chosen to minimize MSE over the Kodak suite, then rounded
to dyadic rationals so the filters run in integer arithmetic with bit-shifts:

```
α = 1/2,   β = 5/8,   γ = 3/4
```

"The filters approximate the optimal Wiener filters within 5%… for a 5×5 support." The
eight 5×5 filters can be applied as a single integer convolution; e.g. red at a green
site in a red row (coefficients ×16, IPOL Eq.):

```
R = ( F(i,j−2) + F(i,j+2)
      − 2·(F(i−1,j−1)+F(i+1,j−1)+F(i−2,j)+F(i+2,j)+F(i−1,j+1)+F(i+1,j+1))
      + 8·(F(i−1,j)+F(i+1,j))
      + 10·F(i,j) ) / 16
```

Malvar beats bilinear by ~4 dB PSNR and is competitive with far more complex methods
(IPOL: Malvar PSNR 29.66 vs bilinear 29.47 vs Hamilton-Adams 29.17 on Kodak-7), but it
is **not directional**, so it still zippers and false-colors on the hardest edges — which
is why directional methods (RCD/AMaZE) win on real data. For an astro pipeline Malvar is
a reasonable *fast, exactly-linear* option that beats bilinear without the photometric
risk of a nonlinear adaptive method; lumos does not implement it (it uses RCD), but it is
the right mental baseline for "what does a purely linear demosaic look like."

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
| **AMaZE** (Aliasing Minimization & Zipper Elimination) | slow | **state of the art** detail | minimal zipper; can color-overshoot on low-contrast/noisy areas | **RawTherapee's default**; best on clean low-ISO data |
| **RCD** (Ratio-Corrected Demosaicing) | fast-medium | near-AMaZE detail | **excellent on round edges / stars**, less overshoot than AMaZE | **darktable's default** (RawTherapee defaults to AMaZE) |

**Operating principles** (what each family actually does, for context on the table):

- **VNG (Variable Number of Gradients)** computes 8 directional gradients around each
  pixel, sets a threshold from their min/max, and averages *only* the neighbors whose
  gradient falls below threshold — so the number of contributing directions varies per
  pixel (hence the name). Robust and maze-free because it averages broadly, but that
  same averaging *loses detail*. Its niche survival is fixing green-channel imbalance
  from lens crosstalk (VNG4 treats G1/G2 separately).
- **AHD (Adaptive Homogeneity-Directed, Hirakawa & Parks 2005)** interpolates the image
  *twice* — once assuming horizontal edges, once vertical — converts both candidates to
  a perceptually uniform space (CIELab), and at each pixel picks the direction whose
  local neighborhood is **more homogeneous** (smaller color-difference variation in a
  small ball). The homogeneity test is what kills both zipper (wrong-direction
  candidates are inhomogeneous) and maze (the decision is spatially smoothed). It is the
  conceptual ancestor of Markesteijn's homogeneity voting; "old, slow, inferior" today
  (RawPedia) only relative to AMaZE/RCD.
- **AMaZE (Aliasing Minimization & Zipper Elimination)** is a more elaborate directional
  method that estimates per-pixel edge direction *and* explicitly models/cancels
  aliasing in the color-difference signal, achieving the sharpest detail of the classic
  set; the cost is occasional **color overshoot** (ringing into colored over/undershoot)
  on low-contrast or noisy regions — precisely the regime of faint nebulosity and noisy
  subs.
- **LMMSE / IGV** are tuned for noisy input: LMMSE applies a linear-MMSE estimate to the
  color-difference planes (suppressing maze on high-ISO), IGV uses an integrated
  gradient with strong directional averaging (strong moiré/maze suppression). Both trade
  some detail for stability — attractive for single noisy subs, less so once temporal
  stacking is the real denoiser.

The two state-of-the-art interior algorithms are **AMaZE** and **RCD**. RawPedia's
own wording: AMaZE "yields the best results in most cases" but "is also more prone to
color overshoots than RCD," while RCD "does an excellent job for round edges, for
example **stars in astrophotography**, while preserving almost the same level of
detail as AMaZE." That last sentence is why RCD is the natural Bayer choice for an
astro pipeline (point sources are round high-contrast edges, exactly where AMaZE's
overshoot would ring as colored halos) and why lumos implements RCD as its primary
Bayer path.

**Correction (pass 3):** the prior draft labeled RCD "RawTherapee's current default" and
AMaZE "RawTherapee's historical default." That is wrong. RawPedia states plainly that
"**AMaZE … is the default demosaicing method**" for RawTherapee (current and historical);
**RCD is darktable's default** (it replaced PPG — darktable's manual: "RCD now offers
similar performance to PPG, but with better results, it is now the default algorithm").
A standing RawTherapee feature request (Beep6581/RawTherapee #5908, "use RCD as default")
confirms RT had *not* switched. The astro takeaway is unchanged — RCD is still the right
Bayer choice for stars on the strength of the round-edge/overshoot behavior above,
independent of which app ships it as the default.

**How RCD works** (`.tmp/refs/librtprocess/src/demosaic/rcd.cc`, originally
LuisSR/RCD-Demosaicing, MIT, by Luis Sanz Rodríguez). The four steps, with the exact
math read out of the canonical source (verified pass 2):

*Step 1 — directional discrimination (`rcd.cc:131-160`).* For each pixel form a
**second-difference high-pass response** along the vertical axis and square it:

```
hpf_V = (cfa[−3w] − cfa[−w] − cfa[+w] + cfa[+3w]) − 3·(cfa[−2w] + cfa[+2w]) + 6·cfa[0]
V_stat = max(epssq, hpf_V[−w]² + hpf_V[0]² + hpf_V[+w]²)        // 3-tap vertical sum
```

(`w` = one row; the horizontal `H_stat` is the same filter rotated 90°). The directional
statistic is the **ratio**

```
VH_Dir = V_stat / (V_stat + H_stat)         ∈ [0,1]
```

This is a *normalized* discriminator: because it is a ratio of like high-pass energies,
a uniform chroma/illumination scaling cancels, which is what makes it robust to
chromatic aberration. `VH_Dir → 0` means "edge runs vertically, interpolate vertically";
`→ 1` the opposite; `≈ 0.5` means no strong direction.

*Step 2 — low-pass filter (`rcd.cc:162-169`).* A 3×3 binomial low-pass of the raw CFA,
mixing whatever colors land in the window:

```
lpf[0] = cfa[0] + ½·(cfa[±w] + cfa[±1]) + ¼·(cfa[±w±1])
```

*Step 3 — green at R/B sites (`rcd.cc:171-200`) — the defining idea.* The missing green
is estimated from each cardinal neighbor by a **ratio correction in the low-pass
domain** (not the usual color-difference domain):

```
N_Est = cfa[−w] · 2·lpf[0] / (eps + lpf[0] + lpf[−w])      // and S/E/W analogously
```

i.e. the neighboring green sample is rescaled by the ratio of the *local low-pass level
at the center* to the *average low-pass level center↔neighbor*. The four cardinal
estimates are blended into a vertical and a horizontal estimate weighted by **opposite**
gradient strength (so the smoother side dominates):

```
V_Est = (S_Grad·N_Est + N_Grad·S_Est) / (N_Grad + S_Grad)
H_Est = (W_Grad·E_Est + E_Grad·W_Est) / (E_Grad + W_Grad)
G = intp(VH_Disc, H_Est, V_Est)            // linear blend by the refined direction
```

where `VH_Disc` is `VH_Dir` further refined against its 4-diagonal-neighbor mean (whichever
of center/neighborhood is *farther from 0.5*, i.e. more confidently directional, wins).
It is this ratio-in-LPF-domain estimate — rather than an additive color difference — that
suppresses the pixel **overshoot** which would otherwise ring as colored speckle around
bright point sources, the property that makes RCD good on stars.

*Step 4 — red/blue (`rcd.cc:202-296`).* With green now complete, R and B are filled at
opposing CFA sites from **diagonal color differences** `rgb[c]−green` blended by a P/Q
diagonal direction statistic (same ratio-of-HPF-energy form, `rcd.cc:206-219`), then at
green sites from **cardinal color differences**, both gradient-weighted exactly as the
green step.

**Correction (pass 2):** the prior draft gave the green estimate as
`G_est = G·(1 + (LPF₀−LPF₂)/(LPF₀+LPF₂))`. That formula does **not** appear in the
reference and is wrong; the actual per-direction estimate is
`N_Est = cfa[N]·2·lpf₀/(eps + lpf₀ + lpf_N)` (algebraically a different quantity — it is
`G·2L₀/(L₀+L_N)`, not `G·(1+(L₀−L₂)/(L₀+L₂))`), combined with gradient-weighted V/H
blending and the `VH_Disc` direction refinement above. lumos's
`src/raw/demosaic/bayer/rcd.rs:227-249` is a faithful SIMD port of the corrected math
(`n_est = cfa[idx-w1]·two_lpfi/(EPS + lpfi + lpf[idx-w2])`); it stores the LPF at full
resolution where the reference half-packs it (`lpindx = indx/2`), so its `lpf[idx − w2]`
equals the reference's `lpf[lpindx − w1]`. The `eps = 1e-5` / `epssq = 1e-10`
divide-by-zero guards (`rcd.cc:72-73`) must be ported verbatim.

### 3.2 X-Trans: Markesteijn

Fuji X-Trans uses a 6×6 CFA (not 2×2), engineered to suppress moiré without an AA
filter — at the cost of making chroma reconstruction much harder. The reference
algorithm is **Markesteijn** (Frank Markesteijn, integrated into dcraw/libraw and
RawTherapee `xtrans_demosaic.cc`). The pipeline, read out of
`.tmp/refs/librtprocess/src/demosaic/markesteijn.cc` (verified pass 2):

1. **Hex lookup + green bounds (`markesteijn.cc:171-360`).** Because X-Trans has no
   regular 2×2 cell, the algorithm precomputes, per CFA phase, a *hexagonal neighbor
   map* (`allhex`) of the nearest green sites around each pixel — lumos materializes this
   as `hex_lookup.rs`. It also clamps each interpolated green to the local
   `[greenmin, greenmax]` of its hex neighbors to bound overshoot.
2. **Directional green (`markesteijn.cc:372-426`).** Green is interpolated in
   `ndir = 4 << (passes > 1)` directions — **4 for 1-pass** (horizontal, vertical, two
   diagonals), **8 for 3-pass** — using fixed hex weights. The exact weights (verified to
   match lumos `markesteijn_steps.rs:286-296`):
   `color = 0.6796875·(g_a + g_b) − 0.1796875·(g_a2 + g_b2)` for one axis and
   `0.87109375·g3 + 0.12890625·g2 + 0.359375·(center − same_color_neighbor)` for the
   cross term.
3. **Iterate green + R/B (`markesteijn.cc:428-…`, the `for pass` loop).** For passes ≥ 2
   the green is *recomputed* from the now-interpolated closer pixels (this is what
   "3-pass" iterates — the green refinement, not merely the direction selection), then R
   and B are filled per direction from color differences.
4. **Homogeneity voting (`markesteijn.cc:728-870`).** Each directional candidate is taken
   to CIELab, per-direction derivatives `drv` are formed, and a per-pixel
   **homogeneity map** counts how many neighbors have `drv ≤ 8·min_drv` (lumos
   `markesteijn_steps.rs:766-820` uses exactly the `threshold = 8 × min` rule). A 5×5 sum
   of the homogeneity maps selects the most homogeneous direction(s) at each pixel.
5. **Blend (`markesteijn.cc:867-…`).** The final RGB averages the qualifying (most
   homogeneous) directions — directly analogous to AHD, generalized to the hex geometry.

lumos implements Markesteijn **1-pass** (4 directions) across `markesteijn.rs` +
`markesteijn_steps.rs` + `hex_lookup.rs`, recomputing RGB on-the-fly in the blend rather
than materializing all `ndir` RGB candidates (memory ~10·P f32).

**Correction (pass 2):** the prior draft said 3-pass "iterates the direction selection
three times." More precisely, multi-pass doubles the direction count (`ndir` 4→8) and
re-derives green from interpolated neighbors on each pass; the homogeneity-based
*selection* runs once at the end. For X-Trans, false color in high-spatial-frequency
luminance is the dominant artifact; the homogeneity voting is what limits it. On *noisy*
astro subs the 1-pass/3-pass quality gap largely vanishes (the noise dominates the
direction/derivative estimates), so 1-pass is the pragmatic astro choice — which is what
lumos picks.

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

Three structural alternatives sidestep demosaic artifacts entirely. Quantitative
tradeoffs (resolution kept, whether neighboring output pixels share input samples →
**noise correlation**, and photometric validity):

| Method | Output resolution | Noise correlation between output px | Photometric validity | Color fidelity / cost |
|--------|-------------------|-------------------------------------|----------------------|------------------------|
| **Full demosaic** (RCD/AMaZE) | full (W×H) | **high** — each output px is a weighted blend of many inputs; adjacent px share inputs | **degraded** — interpolation moves flux; only ⅓ (R/B ¼) of each channel is real, rest is inferred | best preview color; data-dependent → hardest to error-propagate |
| **Superpixel (2×2→1)** | **¼** (½W × ½H) | **none** — each 2×2 cell is disjoint; output px share no inputs | **exact** — pure binning of real samples; a fixed linear map per channel; flux conserved | true measured color; trivial, fastest |
| **Split-CFA** | ¼ per channel, on offset grids | none within a channel | exact (each channel is real samples) | channels misregistered by ½px (must co-register); good for independent per-channel work |
| **CFA / Bayer drizzle** | tunable (scale 1→2) | **low** — drops are placed without interpolation; dithering decorrelates | **exact in the limit** — each drop carries one real photosite's flux | needs many dithered subs; restores resolution debayering can't |

1. **Bayer / CFA drizzle** (Fruchter & Hook drizzle applied to the *mosaic*). Each
   raw photosite is dropped into the output channel its filter color dictates;
   sub-pixel dithering between many frames fills the gaps. This *never interpolates
   color* and so "avoids the artefacts that occur with all debayering algorithms, which
   gives improved noise characteristics" (Siril docs, verified pass 2,
   `.tmp/papers/siril_drizzle.txt`). Requirements: **the CFA pattern must be preserved
   through stacking** (do *not* debayer before drizzle), and frames must be **dithered**.
   Siril's explicit guidance on parameters (verified pass 2): "For OSC drizzle, start with
   scale = pixel fraction = 1.0" — because each color drops only ¼ (R/B) of the input
   pixels onto the output grid, so coverage is already sparse: "a 2x upscale drizzle of a
   color channel from a CFA image has the same amount of reduction in droplet coverage as
   a 2x upscale drizzle [of mono]. If you upscale on top of that, you need as much droplet
   coverage as you would for a 4x upscale drizzle! Therefore it is generally recommended
   to drizzle CFA images at scale = 1." With some kernels it helps to set `pixfrac ≈
   1/scale`. lumos already has a drizzle stage (`src/drizzle/`); feeding it CFA data is
   the natural astro path to a demosaic-free OSC pipeline.
2. **Superpixel** (a.k.a. half-resolution / `bin 2×2`): collapse each 2×2 Bayer cell
   to one output RGB pixel (R, average-of-2-G, B). Halves linear resolution (quarters
   pixel count) but is *exactly* linear, artifact-free, **noise-uncorrelated** (disjoint
   cells), and trivially correct photometrically — ideal for well-sampled or
   short-focal-length OSC data, oversampled setups, or as a fast, trustworthy preview and
   photometric ground-truth against which to validate the RCD path.
3. **Split-CFA / channel extraction**: treat each color's photosites as three
   independent sub-images (each at half resolution, on its own grid) and process them
   like three mono channels. The R/G/B grids are spatially offset by ½ a CFA cell, so
   they must be co-registered before recombination. Useful when you want fully
   independent per-channel registration/stacking with zero color mixing.

**Why chroma smoothing harms photometry, quantitatively.** Photometry measures flux =
Σ(pixel − background) inside an aperture. A linear demosaic (or none) keeps that sum an
unbiased estimate of the channel's collected electrons. A **chroma median** replaces a
pixel's color-difference by a neighborhood order statistic — a *nonlinear, non-flux-
conserving* operator: it neither preserves the aperture sum nor commutes with background
subtraction, so the measured magnitude of a colored star shifts by an amount that depends
on the surrounding pixels (their values, their count, the kernel). On a sparse star field
the "neighborhood" of a red star is mostly dark sky, so the median pulls the star's chroma
toward neutral — desaturating it and corrupting any color-index measurement. There is no
calibration that undoes a spatially-varying nonlinear filter; the only safe choice is not
to apply it.

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
4. Keep white balance at unity. Retain camera multipliers **min-normalized to 1.0**
   as metadata for optional later color calibration, but never apply them during
   decode. Never apply `output_color`, gamma, or auto-bright.
5. **Calibrate on the mosaic**: dark/bias subtract, divide by the prepared flat whose
   divisor uses **per-CFA-color means**, then defect-correct against a
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
  per-CFA-color means (lumos prepares and caches these in `calibration_masters::prepared_flat`).
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
- **Feeding gamma-encoded 8-bit PNG/JPEG (or unknown-provenance TIFF) as science data.**
  These are display artifacts: sRGB-gamma-encoded, already demosaiced, and (JPEG) lossy.
  Loaded as-is they break `pixel ∝ photons`; the gamma is invertible but the 8-bit
  quantization and JPEG loss are not. Use them only for previews; accept 16-bit TIFF only
  when known linear (§2.6).

---

## 6. How lumos currently does it — and gaps/opportunities

**FITS** (`src/io/astro_image/fits.rs`): solid. `fits-well` applies `BZERO`/`BSCALE`,
endianness, and integer `BLANK`; the loader selects the first image-bearing HDU, including
tile-compressed images. It handles 2-D/3-D axis order and planar RGB correctly; composes
`ROWORDER` + `XBAYROFF` + `YBAYROFF` + `BAYERPAT` for OSC FITS; parses RA/Dec in all three
common encodings; preserves physical float values and rejects null/non-finite samples with
a summary until the image model has a validity plane.

**RAW** (`src/raw/mod.rs`): strong. `consolidate_black_levels` is a faithful port of
libraw `adjust_bl()` including the 2×2/X-Trans spatial folding, common-minimum
extraction, and per-channel deltas. Camera WB stays at unity for direct and calibration
workflows. Sensor dispatch picks our fast RCD (Bayer) / Markesteijn-1-pass (X-Trans)
and falls back to libraw configured for **linear**, unity-WB output (`gamm={1,1}`,
`output_color=0`, `no_auto_bright`). The separate `load_raw_cfa` path
returning an un-demosaiced `CfaImage` is exactly right and enables the
calibrate-then-debayer ordering, with flat division using per-CFA-color means.

**Gaps / opportunities:**

1. **No native CFA-drizzle wiring.** lumos has both a drizzle stage and an
   un-demosaiced `CfaImage`, but they aren't connected into a CFA/Bayer-drizzle path.
   Given RawPedia/Siril guidance that CFA drizzle is the artifact-free OSC ideal, a
   `drizzle_stack` variant that consumes mosaics (scale = pixfrac = 1.0, dithered) is
   a high-value addition.
2. **No superpixel / split-CFA mode.** A trivial, exactly-linear half-res debayer
   would be a useful fast/trustworthy alternative and a photometric ground truth for
   testing the RCD path.
3. **No DNG linearization-table handling (pass 2).** lumos assumes post-unpack
   linearity; a sensor shipping a DNG `LinearizationTable` would be mis-handled (§2.4).
   Mainstream astro CMOS is natively linear so this is low-priority, but worth noting.
4. **Defect correction is dark-derived (good) but lives in the calibration module**,
   not the load module — fine architecturally, but the load docs should make the
   "defects corrected on the mosaic before demosaic" ordering explicit so callers
   don't accidentally demosaic the uncalibrated `load_raw` output for OSC data.
5. **Standard formats are loaded without a linearity check (pass 3).** `from_file`
   (`mod.rs:276-287`) routes TIFF/PNG/JPEG to imaginarium with no gamma decode and no
   linear/nonlinear provenance check (§2.6). An 8-bit sRGB JPEG fed as stacking input
   silently violates `pixel ∝ photons`. Consider refusing 8-bit lossy formats as
   stacking input, and at minimum documenting/optionally applying inverse-sRGB at the
   API boundary.

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
  min-normalization that lumos deliberately does not apply.
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
- LuisSR RCD-Demosaicing — <https://github.com/LuisSR/RCD-Demosaicing> — RCD origin
  (no formal paper); green estimate = directional LPF-domain ratio
  `cfa[N]·2·lpf₀/(eps+lpf₀+lpf_N)`, CA-invariant `V_stat/(V_stat+H_stat)` direction
  statistic, `VH_Disc` neighborhood refinement (math read from `rcd.cc`, see §3.1).
- Siril drizzle docs — <https://siril.readthedocs.io/en/latest/preprocessing/drizzle.html>
  — CFA/Bayer drizzle for OSC: scale = pixfrac = 1.0, preserve CFA, no debayer before
  drizzle, avoids debayering artifacts.
- PixInsight forum, "Bayer drizzle instead of de-Bayering" —
  <https://pixinsight.com/forum/index.php?threads/bayer-drizzle-instead-of-de-bayering-with-osc.12996/>
  — practical CFA-drizzle rationale and green-bias correction.
- darktable demosaic manual — <https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/demosaic/>
  — RCD/AMaZE/VNG/Markesteijn behavior; states **RCD is darktable's default** (replaced
  PPG): "RCD now offers similar performance to PPG, but with better results, it is now the
  default algorithm" — and AMaZE "is also more prone to color overshoots than RCD."
- RawTherapee issue #5908, "Suggestion to use RCD as default demosaic method" —
  <https://github.com/Beep6581/RawTherapee/issues/5908> — confirms RawTherapee's default is
  **AMaZE, not RCD** (pass-3 correction; RawPedia: "AMaZE … is the default demosaicing method").
- astropy CCD Data Reduction Guide / ccdproc — <https://www.astropy.org/ccd-reduction-and-photometry-guide/>
  and <https://ccdproc.readthedocs.io/en/latest/reduction_toolbox.html> — canonical
  calibration order: overscan → trim → bias → dark (scaled) → flat; debayer separate.
- "A Review of an Old Dilemma: Demosaicking First, or Denoising First?" —
  <https://arxiv.org/pdf/2004.11577> — the unresolved order debate (moot for astro,
  where temporal stacking is the denoiser).
- Clark, "Astrophotography Image Processing Using Modern Raw…" —
  <https://clarkvision.com/articles/astrophotography.image.processing/> — keeping the
  raw pipeline linear and the DN→flux ordering.

### Primary sources parsed (pass 2)

PDFs successfully downloaded and extracted with `pdftotext`, then quoted above:

- **FITS Standard 4.0** (full 75-page standard, `fits_standard40aa-le.pdf`) →
  `.tmp/papers/fits40.txt`. Takeaway: confirmed verbatim the scaling equation (§4.4.2
  Eq. 3), the unsigned-integer BZERO offsets (Table 11, §5.2.5), the raw-value `BLANK`
  rule + uint16 `BLANK=−32768` example (§4.4.2.5), `BLANK` forbidden / NaN as float
  blank (§5.3), big-endian two's-complement storage (§3.3.2), and the entire tiled
  image-compression machinery (ZIMAGE/ZCMPTYPE/ZTILEn/ZSCALE/ZZERO/ZQUANTIZ subtractive
  dithering + photometric-bias warning, §10) and WCS-keyword preservation (§10.1.2).
- **Getreuer 2011, "Malvar-He-Cutler Linear Image Demosaicking"** (peer-reviewed IPOL
  reproduction of Malvar et al. 2004, `article.pdf`) → `.tmp/papers/malvar_ipol.txt`.
  Takeaway: the exact gradient-corrected linear formula (bilinear + Laplacian
  cross-channel term), gains α=½, β=⅝, γ=¾, the integer ×16 convolution kernel, and the
  Kodak PSNR comparison vs bilinear/Hamilton-Adams. Used as the better-than-original
  primary source for §3.0 (the Microsoft-hosted original 403'd).
- **Siril drizzle documentation** (HTML, converted) → `.tmp/papers/siril_drizzle.txt`.
  Takeaway: the quantitative OSC/CFA-drizzle guidance ("start with scale = pixel
  fraction = 1.0"; why CFA upscaling needs disproportionately more subs; CFA drizzle
  "avoids the artefacts that occur with all debayering algorithms"). Used to make §3.3
  quantitative.

Failed / unavailable (tried multiple mirrors, all 403'd or returned HTML):

- Malvar et al. 2004 original ICASSP PDF (Microsoft Research) — superseded by the IPOL
  reproduction above, which carries the same math peer-reviewed.
- Hirakawa & Parks 2005 (AHD) original PDF — AHD principle reconstructed from the
  RawTherapee/librtprocess `ahd.cc` source and RawPedia instead.
- Li/Gunturk/Zhang 2008 "Image demosaicing: a systematic survey" and Menon & Calvagno
  "Color image demosaicking: an overview" — not retrievable; artifact taxonomy and
  algorithm principles cross-checked against RawPedia + the cloned RawTherapee sources.

Still unverifiable from a primary source: there is **no formal published RCD paper**
(RCD is defined by its reference C code + RawPedia); the §3.1 RCD math is therefore
verified against the canonical implementation `librtprocess/.../rcd.cc` rather than a
paper — which for an algorithm-as-code is the authoritative source.
