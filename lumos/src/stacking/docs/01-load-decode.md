# Stage 1 — Load, decode, and demosaic

This document specifies how Lumos turns an input file into either a calibrated-ready
single-plane CFA image or a planar linear-light image. It is both an implementation
contract and an audit of the current code.

The loader is the first scientific boundary in the stacking pipeline. A mistake here
cannot be repaired by registration or stacking: a wrong black level biases every
exposure, a one-row CFA phase error swaps colors, a transfer-encoded PNG changes
photometry, and clipping destroys negative noise that calibration needs.

Normative terms have their usual meanings:

- **MUST** is required for correctness.
- **SHOULD** is the preferred behavior unless a documented input constraint justifies
  another choice.
- **MAY** is optional.

## 1. Contract and data domains

### 1.1 Two products, not one

Loading has two distinct products. They must remain distinct in the API.

#### Scientific CFA product

Use this for mosaic RAW and mosaic FITS frames that will be calibrated.

`CfaImage` contains:

- one row-major `f32` sample plane;
- active width and height;
- a Bayer 2 × 2 or X-Trans 6 × 6 color map expressed at the active-image origin;
- the normalization transform and quantization step;
- enough acquisition metadata to reject incompatible calibration frames;
- optional validity and saturation masks.

Samples are linear sensor values. They MAY be negative after black subtraction or
calibration and MAY exceed nominal saturation after arithmetic. They MUST NOT be
clamped merely to make them fit `[0, 1]`.

#### Linear image product

Use this for already-monochrome data, already-demosaiced linear RGB, or a deliberately
developed preview.

`LinearImage` contains one or three planar `Buffer2<f32>` channels. Its numeric domain
depends on provenance:

| Provenance | Required domain |
|---|---|
| FITS physical image | finite physical values in `BUNIT` or unspecified units; no universal range |
| calibrated CFA after demosaic | linear camera/sensor RGB; negative and >1 values are valid |
| direct RAW preview | normalized and usually clipped to `[0, 1]` |
| explicitly decoded standard image | linear-light values, normally normalized to `[0, 1]` |

The type or metadata must retain this provenance. Code must not infer scientific
linearity merely because storage is `f32`.

### 1.2 Linearity policy

The following distinctions are essential:

- byte decoding, `BSCALE`/`BZERO` application, fixed black subtraction, and division
  by a fixed acquisition scale are affine operations;
- white-balance multiplication and a 3 × 3 color matrix are linear operations, but
  they still belong after calibration because they change channel gains, headroom,
  noise weighting, and the meaning of the sensor channels;
- RCD and Markesteijn are adaptive spatial estimators and therefore nonlinear;
- clipping, gamma/transfer functions, tone curves, local contrast, denoising,
  highlight recovery, and gamut mapping are nonlinear.

The scientific order for a mosaic light frame is:

```text
decode stored samples
  -> apply format-defined linearization
  -> construct a signed CFA plane
  -> calibrate in CFA space
  -> detect/repair defects or cosmic rays as configured
  -> demosaic
  -> registration and stacking in linear sensor-channel space
  -> optional white balance and camera-to-working-space matrix
  -> display transform
```

Demosaicing before CFA calibration is incorrect because a master dark or flat sample
belongs to one sensor photosite, while a demosaiced pixel is a content-dependent
mixture of neighboring photosites.

The alternative CFA-drizzle branch omits per-frame demosaic:

```text
calibrated CFA
  -> obtain registration transforms from a linear registration proxy
  -> drizzle each photosite only into its matching output color plane
  -> normalize coverage and combine frames
  -> optional color transform and display transform
```

Its transform must map original CFA coordinates, not coordinates from a resampled
proxy.

### 1.3 Global invariants

Every successful load MUST establish all of these invariants:

1. `width > 0`, `height > 0`, and every size multiplication is overflow-checked.
2. A mono/CFA image has exactly `width * height` samples; RGB has exactly three such
   planes.
3. Pixel coordinates use one documented internal orientation.
4. The CFA color function addresses the same coordinates as the sample plane.
5. Undefined input samples are represented by a validity mask or cause an explicit
   error; they are never silently converted to zero.
6. All stored output samples are finite. If non-finite physical FITS values are not
   supported downstream, loading fails with the count and first coordinate.
7. Scientific data are not clipped or transfer-encoded.
8. The normalization scale is determined by acquisition metadata or a fixed type
   convention, never by the observed maximum of an individual frame.
9. Native CFA samples survive demosaicing exactly, apart from an explicitly selected
   post-demosaic transform.
10. Cancellation and allocation failures do not publish a partially initialized image.

## 2. Format detection and dispatch

### 2.1 Detection order

The loader SHOULD identify containers by content, with the extension used as a hint:

1. Read a bounded prefix.
2. Detect FITS from the initial `SIMPLE  =` card or a supported FITS container
   signature.
3. Detect TIFF/DNG, PNG, and JPEG from their signatures.
4. Let LibRaw probe formats it supports.
5. Use the extension only to disambiguate or to produce a better diagnostic.

This accepts conventional names such as `.fits.fz` and avoids rejecting cameras that
LibRaw supports merely because their extension is absent from a local list. A
case-insensitive extension-only fast path is acceptable only if a signature check
confirms it before decoding.

Do not confuse `.fits.fz` with `.fits.gz`. An fpack `.fz` file is still a FITS file
whose compressed image is represented by `ZIMAGE` tables and normally begins with a
`SIMPLE` primary HDU. A `.gz` file is an outer gzip stream; support requires bounded
gzip decompression followed by a fresh inner signature/HDU validation. Reject trailing
garbage and enforce both compressed-input and decompressed-output limits.

### 2.2 Dispatch policy

```text
CfaImage::from_file(path, context): camera RAW or validated sensor-plane FITS
LinearImage::from_file(path, context): physical non-mosaic FITS or declared-linear float TIFF
PreviewImage::from_file(path, context): preview-decoded FITS, RAW, TIFF, PNG, or JPEG
```

The product type MUST be explicit. A generic entry point that sometimes produces clipped RGB and
sometimes unbounded physical data is too easy to misuse in calibration.

## 3. FITS

The normative format reference is FITS Standard 4.0. FITS consists of one primary HDU
followed by zero or more extension HDUs. The first HDU is not necessarily the science
image.

### 3.1 Parse the complete HDU structure

For every HDU:

1. Read 80-byte ASCII cards until `END`.
2. Round the header to the next 2880-byte boundary.
3. Parse required structural keywords in their required context.
4. Calculate the data length using checked integer arithmetic.
5. Skip the data plus padding to the next 2880-byte boundary.

Never scan for the next textual `XTENSION` marker inside binary data.

Plain image HDUs are:

- a primary HDU with `NAXIS > 0` and a nonzero data product;
- an `IMAGE` extension.

Tile-compressed images use the FITS tiled-image convention and appear as a
`BINTABLE` with `ZIMAGE = T`. The image reader must expose these with the same logical
sample and shape API as an uncompressed image. Integer Rice/GZIP/PLIO/HCOMPRESS
decoding can be lossless, but floating tiled images may be quantized before integer
compression. Preserve `ZCMPTYPE`, `ZQUANTIZ`, `ZSCALE`, `ZZERO`,
algorithm-specific `ZNAME*`/`ZVAL*` parameters, and dithering metadata. A caller that
requires lossless science input must reject a lossy quantized
image; otherwise attach its quantization/noise provenance. Compression does not change
the logical shape or `BSCALE`/`BZERO` semantics, but it is not always scientifically
lossless.

### 3.2 HDU selection

The API SHOULD accept one of:

- explicit zero-based HDU index;
- `EXTNAME` plus optional `EXTVER`;
- an application-level science-image selector.

Without a selector, a documented compatibility rule MAY select the first image-bearing
HDU, but the chosen HDU index, `EXTNAME`, and `EXTVER` MUST be recorded. Silently
choosing the primary HDU is wrong for common multi-extension and `.fits.fz` files.

If multiple plausible science images exist, an interactive caller should ask for a
choice; a batch caller should fail deterministically unless its policy selects one.

`INHERIT = T` is a convention, not permission to concatenate arbitrary headers.
Metadata lookup should:

1. read the selected extension;
2. if the convention is enabled, inherit only non-structural primary keywords;
3. let extension values override inherited values;
4. never inherit `SIMPLE`, `BITPIX`, `NAXIS*`, `EXTEND`, `PCOUNT`, `GCOUNT`, table
   layout, compression, checksum, or other structural cards.

### 3.3 Shape and memory layout

For an image HDU, `NAXIS1` is the fastest-changing axis. Lumos maps:

- `[NAXIS1, NAXIS2]` to mono `[width, height]`;
- `[NAXIS1, NAXIS2, 1]` to mono;
- `[NAXIS1, NAXIS2, 3]` to planar RGB, with each complete plane contiguous, only
  when a supported convention or an explicit caller choice identifies that axis as
  R/G/B.

Shape alone does not prove that a three-plane cube is color; it may be three times,
wavelengths, Stokes planes, or other measurements. Any cube without declared channel
semantics requires an explicit slice/sequence API. It must not be silently reduced to
the first plane. In particular, `NAXIS3 > 3` is commonly a sequence or spectral cube,
not RGB.

Before allocation:

```text
pixel_count  = checked_mul(NAXIS1, NAXIS2)
sample_count = checked_mul(pixel_count, channels)
byte_count   = checked_mul(sample_count, bytes_per_sample)
```

Reject zero axes, unsupported axis counts, products that exceed `usize`, and products
that exceed configured memory limits.

### 3.4 Stored numeric types

The image `BITPIX` values are:

| `BITPIX` | Stored representation |
|---:|---|
| 8 | unsigned 8-bit integer |
| 16 | big-endian signed 16-bit integer |
| 32 | big-endian signed 32-bit integer |
| 64 | big-endian signed 64-bit integer |
| -32 | big-endian IEEE-754 binary32 |
| -64 | big-endian IEEE-754 binary64 |

FITS does not have native unsigned 16/32/64 image types. Canonical unsigned images use
`BSCALE = 1` and `BZERO = 2^(n-1)` with the corresponding signed `BITPIX`. A signed
8-bit logical image can similarly use `BITPIX = 8` and `BZERO = -128`.

The decoder MUST preserve the logical distinction among `I8`, `U8`, `I16`, `U16`,
`I32`, `U32`, `I64`, and `U64` when it reports provenance. Collapsing `I8` into
`U8` or `U64` into `I64` chooses the wrong type range and can corrupt later policy.

`U64` and large `I64` values cannot all be represented exactly by `f32` or `f64`.
If exact integer identity matters, retain an integer view. If the pipeline requires
`f32`, narrow only after physical scaling and report that the final representation is
lossy. Reject a file whose required precision cannot meet the caller's policy.

### 3.5 Nulls and physical-value conversion

Let `r` be the stored array value. For a defined sample:

```text
physical = BZERO + BSCALE * r
```

with defaults `BSCALE = 1` and `BZERO = 0`.

The exact conversion order is:

1. Decode the stored value with the `BITPIX` type and big-endian byte order.
2. For integer images, compare the unscaled stored integer against `BLANK`.
3. If it equals `BLANK`, mark the sample undefined and do not scale it.
4. For floating images, treat IEEE NaN as undefined; handle infinities according to
   the validity policy.
5. Otherwise evaluate `BZERO + BSCALE * r` in `f64`.
6. Check that the physical result is finite.
7. Narrow to `f32` once, at the Lumos buffer boundary.

`BLANK` is a code in the stored integer domain, not a physical value. Comparing it
after scaling is a format error. `BLANK` has no defined null role for floating images.

For an unquantized integer image, the physical one-code quantization step is
`abs(BSCALE)` and its ideal uniform quantization sigma is
`abs(BSCALE)/sqrt(12)`. If an explicit stable normalization later divides the physical
plane by `S`, divide both values by `abs(S)`. Tiled floating-point quantization has its
own tile scale/dither model and must not be summarized as ordinary `BSCALE`
quantization.

A validity mask is the most general representation. Until all downstream algorithms
honor such a mask, Lumos SHOULD reject an image containing undefined/non-finite samples
and report the HDU, total count, and first `(x, y, channel)`. Replacing them by zero
would create false black pixels and stacking artifacts.

### 3.6 No `DATAMAX` normalization

`DATAMIN` and `DATAMAX` describe the minimum and maximum valid **physical values
represented by that array**. They are not the detector black and white levels, not a
container type bound, and not a stable scale shared by sibling frames.

Therefore:

```text
wrong: output = physical / DATAMAX
right: output = physical
```

Using `DATAMAX` as a divisor changes the gain independently for every light, dark,
bias, and flat. A frame whose brightest star changes then changes every pixel, which
invalidates calibration and photometric comparison.

If a later algorithm truly requires normalized FITS samples, normalization must be an
explicit operation with a recorded, stable scale:

- an acquisition-system full-scale value shared by the whole set;
- a declared detector white level;
- or a fixed logical type convention agreed by the caller.

It must never use an observed per-image extremum. `DATAMIN`/`DATAMAX` remain useful for
validation and display defaults.

### 3.7 Checksums

`CHECKSUM` covers the HDU and `DATASUM` covers its data records. The reader SHOULD
offer:

- `Ignore` for trusted local performance-sensitive work;
- `VerifyIfPresent` as the normal default;
- `RequireValid` for archival or transferred inputs.

Checksum failure is an input error, not a warning that permits scientific processing.
Record whether each selected HDU was unchecked, absent, or verified.

### 3.8 Orientation and coordinates

FITS specifies array indexing and recommends conventions, but it does not universally
mandate that the first stored row is a camera's top or bottom sensor row. The historic
lower-left display convention is a recommendation, not sufficient CFA metadata.

Lumos should use top-left, top-down internal image coordinates:

- `x = 0` is the leftmost internal column;
- `y = 0` is the top internal row;
- index = `y * width + x`.

WCS must be transformed with the pixels if rows or columns are physically reversed.
For a vertical reversal of height `H`:

```text
y_source = H - 1 - y_destination
```

In FITS's one-based WCS pixel coordinates, write the source-to-destination relation as:

```text
p_source = A * p_destination + b
A = [[1, 0], [0, -1]]
b = [0, H + 1]
```

For a linear WCS `world_delta = CD * (p_source - CRPIX_source)`:

```text
CD_destination = CD_source * A
CRPIX_destination = inverse(A) * (CRPIX_source - b)

therefore:
CRPIX2_destination = H + 1 - CRPIX2_source
the second column of CD changes sign
```

The same affine composition generalizes to horizontal flips and transposition. For
`PC` + `CDELT` headers, either update the equivalent matrix consistently or let the
WCS library perform the coordinate transform; changing only `CDELT2` is insufficient
when rotation/skew terms exist. Never flip pixels for display while leaving the WCS
and CFA phase untouched.

### 3.9 FITS Bayer metadata

`BAYERPAT`, `XBAYROFF`, `YBAYROFF`, `ROWORDER`, and `COLORTYP` are ecosystem
conventions rather than core FITS image keywords. Parse case-insensitively after
trimming FITS string padding.

Accepted Bayer strings are `RGGB`, `GRBG`, `GBRG`, and `BGGR`. Define the parsed
2 × 2 pattern `P` so:

```text
P[0][0] P[0][1] = first two characters
P[1][0] P[1][1] = last two characters
color_file(y, x) = P[(y + YBAYROFF) mod 2][(x + XBAYROFF) mod 2]
```

Offsets must use Euclidean modulo so negative values, if accepted, behave
deterministically. Unsupported strings are errors when demosaicing is requested.

`ROWORDER` values in current astronomy software are `TOP-DOWN` and `BOTTOM-UP`.
Because this keyword is non-standard and sometimes absent, the fallback must be a
visible preference or acquisition-profile setting, not an undocumented guess.

If a bottom-up sample plane of height `H` is physically reversed into top-down order,
the CFA phase is:

```text
color_top_down(y, x) =
    color_file(H - 1 - y, x)

vertical_phase_shift = (H - 1) mod 2
```

Consequently:

- an even-height reversal swaps the two Bayer pattern rows;
- an odd-height reversal leaves their parity unchanged.

An unconditional `flip_vertical()` on the pattern is wrong for odd heights. Apply the
pixel transform, the `H - 1` phase, and `XBAYROFF`/`YBAYROFF` exactly once. Siril's
public orientation fixture set covers the combinations and should be mirrored in
Lumos tests.

Some writers use `COLORTYP` instead of or in addition to `BAYERPAT`. If both are
present and disagree after normalization, fail with both values rather than guessing.
`BAYERPAT = 'TRUE'` is a vendor convention used by some files; supporting it as RGGB
must be an explicit compatibility rule and tested with that producer.

For an X-Trans FITS convention, require a complete 36-character row-major R/G/B
pattern rather than trying to infer it from a four-character Bayer keyword. Apply
`XBAYROFF` and `YBAYROFF` modulo six, validate the 8/20/8 color counts and green
neighborhoods, and transform a vertical reversal with:

```text
color_top_down(y, x) =
    P[(H - 1 - y + YBAYROFF) mod 6][(x + XBAYROFF) mod 6]
```

This produces a vertical phase shift of `(H - 1) mod 6`. Do not reduce X-Trans
orientation to the even/odd Bayer rule.

### 3.10 FITS metadata

Store the complete selected header or a lossless normalized representation, then expose
typed fields. At minimum retain:

- selected HDU index, `EXTNAME`, and `EXTVER`;
- `BITPIX`/logical sample type, `BSCALE`, `BZERO`, `BUNIT`, `BLANK` status;
- `DATE-OBS` and a normalized exposure duration;
- instrument, detector, telescope, filter, gain, offset, temperature, binning;
- CFA pattern, offsets, row order, and the resulting internal pattern;
- WCS cards and celestial coordinates;
- checksum state;
- history/comment provenance when output is later written.

Exposure aliases need an explicit precedence table. For example, accept `EXPTIME` then
`EXPOSURE` only when their values are finite and positive; if both exist and disagree
beyond a documented tolerance, report ambiguity.

Parse observation time with the FITS time-coordinate rules rather than the host locale:

- retain `TIMESYS` and the original `DATE-OBS`/`MJD-OBS`/`JD-OBS` keyword;
- distinguish exposure start, midpoint, and end when `DATE-BEG`, `DATE-AVG`, or
  `DATE-END` is available;
- preserve subsecond precision;
- do not append a local UTC offset to a zone-less FITS timestamp;
- if two normalized time representations disagree beyond their declared precision,
  report the conflict.

Never parse right ascension with one unconditional unit assumption. A numeric `CRVAL1`
with an RA axis is degrees; a sexagesimal `OBJCTRA` is commonly hours. Preserve the
source keyword and unit alongside the normalized radians/degrees.

## 4. Camera RAW and DNG

Lumos currently binds LibRaw 0.20.1 through `libraw-rs-sys`. Its behavior must be
checked against that pinned source, not inferred from a newer GUI application's
defaults.

### 4.1 Decoder boundary

The RAW decoder must produce or expose:

- full raw dimensions and active-area margins;
- unpacked sensor samples before white balance, demosaic, color conversion, gamma,
  auto-brightness, and clipping;
- sensor layout: mono, Bayer, X-Trans, or unsupported;
- the CFA pattern at the full-raw origin;
- scalar, per-channel, and repeating black metadata;
- white/saturation level metadata;
- linearization information and whether it was applied;
- camera white balance as metadata only;
- camera-to-XYZ/working-space matrices as metadata only;
- exposure, ISO, timestamp, temperature when present;
- make, model, and unique camera identity.

`raw_image` is preferred to a developed LibRaw image, but “raw” does not guarantee that
no file-defined linearization occurred. DNG `LinearizationTable` and compressed decoder
paths can be applied during unpack. The wrapper MUST define its contract per decoder:

```text
unpacked_sample =
    file-defined linearized code, if the decoder applied the table
    otherwise stored code, with the table retained for Lumos to apply once
```

Never apply a linearization table twice. Add real fixture tests for uncompressed and
losslessly compressed DNGs with a non-identity table before claiming uniform behavior
across LibRaw decoders. Black and white metadata used with the unpacked samples must
be expressed in the same post-linearization domain; use the pinned decoder's documented
level semantics rather than combining stored-code levels with linearized samples.

### 4.2 Active area and CFA origin

Keep full-raw and active coordinates distinct:

```text
raw_x = active_x + left_margin
raw_y = active_y + top_margin
active_width  = iwidth
active_height = iheight
```

For a Bayer pattern `P_raw` defined at the full-raw origin:

```text
P_active(y, x) =
    P_raw[(y + top_margin) mod 2][(x + left_margin) mod 2]
```

For X-Trans, use modulo 6. Do not crop first and continue using the unshifted pattern.
The origin of a repeating black map is a separate decoder contract. LibRaw's
`cblack[6..]` repeat is applied in visible-image coordinates, so for a full-raw
coordinate its phase is:

```text
repeat_y = (raw_y - top_margin) mod repeat_h
repeat_x = (raw_x - left_margin) mod repeat_w
```

Use Euclidean modulo. Do not assume the CFA and repeat maps share an origin merely
because both are periodic.

Validate:

- margins and active dimensions lie inside the raw buffer;
- row stride and sample count cover the declared full dimensions;
- Bayer has a valid 2 × 2 arrangement;
- X-Trans is exactly 6 × 6, has 8 red, 20 green, and 8 blue positions per period,
  and satisfies the neighborhood assumptions required by Markesteijn.

### 4.3 Black-level model

LibRaw can represent a common black, four channel deltas, and a repeating spatial
table. DNG additionally defines full-width and full-height black deltas. The complete
model is:

```text
black(raw_y, raw_x, c) =
    B
    + C[c]
    + T[repeat_y(raw_y)][repeat_x(raw_x)]
    + H[active_x]
    + V[active_y]
```

where:

- `B` is a common base;
- `C[c]` is the residual for sensor color/channel `c`;
- `T` is the residual repeat table, indexed from the origin defined by the decoder;
- `H` and `V` are optional DNG `BlackLevelDeltaH` and `BlackLevelDeltaV` vectors;
- an absent table is equivalent to a 1 × 1 zero table.

A stable consolidation equivalent to LibRaw's `adjust_bl()` is:

1. Validate repeat dimensions and the table length before indexing `cblack`.
2. Fold a 2 × 2 repeat table into the four Bayer channel entries, or a 1 × 1 table
   into every channel, when the pinned LibRaw representation requires it.
3. Let `m_c = min(channel_black[c])` over used channels. Subtract `m_c` from every
   channel black and add `m_c` to `B`.
4. Let `m_t = min(T)`. Subtract `m_t` from every repeat entry and add `m_t` to `B`.
5. The resulting `C` and `T` are residuals; preserve the repeat table's declared
   origin. For LibRaw `cblack` this is the visible origin described in section 4.2.

This rearrangement does not change `black(y,x,c)`. It reduces cancellation and makes
the shared normalization denominator explicit.

LibRaw 0.20.1 averages each DNG `BlackLevelDeltaH`/`BlackLevelDeltaV` vector into its
common black during TIFF parsing; it does not expose the original spatial vectors
through `cblack`. A decoder that promises complete DNG black correction must retain and
apply the vectors instead of replacing them by their mean. Lumos's current LibRaw-only
model can exactly reproduce the metadata LibRaw exposes, but not the discarded
per-column/per-row variation.

Reject invalid repeat dimensions, insufficient table storage, non-finite metadata,
impossible channel indices, and `white <= B`. A malformed camera profile is a decoder
error, not a reason to continue with zero black.

### 4.4 Normalization

For a generic camera RAW path, define one fixed code-range denominator `D` for the
acquisition:

```text
D = white_reference - B
sample = (linearized_raw - black(raw_y, raw_x, c)) / D
```

For current LibRaw compatibility, `white_reference` is `imgdata.color.maximum` before
the common black is removed, so `D = maximum - B`. Do not use the observed maximum
sample in the image. If a future decoder exposes per-channel white levels, the choice
between a shared scale and per-channel scales must be explicit because per-channel
division changes sensor channel gains.

For a conforming DNG linear-reference conversion, follow DNG 1.7.1.0 instead:

```text
linearized =
    stored                                      if LinearizationTable is absent
    table[min(stored, table_length - 1)]        otherwise

q(y, x, p) = BlackLevel(y, x, p)
           + BlackLevelDeltaH[x]
           + BlackLevelDeltaV[y]

q_max[p] = maximum q over the active area of storage sample plane p
D[p] = WhiteLevel[p] - q_max[p]
sample = (linearized - q(y, x, p)) / D[p]
```

The `BlackLevel` repeat begins at the top-left of `ActiveArea`. For an ordinary mosaic
DNG, `SamplesPerPixel=1`, so every CFA color belongs to storage
plane `p=0` and uses the same `D[0]`. Do not mistake CFA colors for separate TIFF
sample planes. Validate every `D[p] > 0`. The DNG rendering model clips values above
one and permits clipping below zero, but the scientific CFA path deliberately omits
both clips to preserve overshoot, saturation evidence, and negative noise.

The scientific CFA path stores `sample` without clipping. Negative values are expected
in bias/dark-subtracted noise. Values above one carry saturation/overshoot information.

The direct preview path MAY use:

```text
preview = clamp(sample, 0, 1)
```

but its type/provenance must state that it is unsuitable for calibration. Clipping
before demosaic and clipping after demosaic are separate policy decisions; neither may
occur in the scientific path.

All calibration inputs in a set must use the same black model semantics and denominator
definition. Preserve `D` and the black model in metadata so compatibility checks can
detect mismatches.

### 4.5 Quantization and masks

For a uniform integer code step of one and denominator `D`:

```text
quantization_step  = 1 / D
quantization_sigma = quantization_step / sqrt(12)
```

After a nonlinear lookup table, the local step is not necessarily uniform. Either
retain a local noise model or mark the scalar quantization estimate approximate.

Maintain masks separately from sample values:

- invalid/unreadable;
- saturated or near-saturated;
- optical black/non-active;
- decoder-reported bad pixels.

A saturated sample should not be hidden by black subtraction or a white-balance
multiplier. Determine saturation in the appropriate preprocessed sensor-code domain
and propagate it through demosaic as a mask.

### 4.6 White balance, color, and LibRaw fallback

For scientific loading:

- do not request auto white balance;
- do not request camera white balance application;
- do not apply `pre_mul`/`cam_mul`;
- do not auto-brighten;
- do not apply a camera/output color matrix;
- do not gamma encode;
- do not denoise or sharpen.

White-balance coefficients and matrices should be retained as metadata.

For a deliberately developed LibRaw fallback, all parameters must be set, because
`output_color = 0` alone does not disable white balance or brightness processing.
A unity, nominally linear fallback config is:

```text
gamm = [1, 1]
bright = 1
user_mul = [1, 1, 1, 1]
use_auto_wb = 0
use_camera_wb = 0
no_auto_bright = 1
output_color = 0
output_bps = 16
```

Even then, the result is a developed preview, not a raw CFA frame. If it has three
channels, metadata must not label it `Mono` merely because the original CFA was
unknown. If the scientific path cannot identify the mosaic, it must return
`UnsupportedCfa`.

## 5. Demosaicing

### 5.1 Common requirements

The demosaicer consumes a calibrated, signed CFA plane and returns planar
linear-camera RGB. It MUST:

- use the pattern at the active origin;
- preserve every native sample in its corresponding output channel exactly;
- accept finite negative and >1 input;
- avoid NaN/Inf even on constant, zero, or sign-changing neighborhoods;
- define every border pixel;
- never apply white balance, a color matrix, gamma, or clipping;
- check cancellation between full-image stages;
- crop margins only after every stencil that intentionally uses them.

Adaptive demosaicing is not flux-conserving in the strict photometric sense. For
precision photometry, retain the calibrated CFA or use an aperture model aware of the
CFA. Demosaicing is still appropriate for registration, visual RGB, and color stacking.

### 5.2 Bayer: RCD

Lumos uses Ratio Corrected Demosaicing for Bayer data. The following is the implemented
signed-domain variant, based on Luis Sanz Rodríguez's RCD and `librtprocess`.

Let `C[i]` be the CFA sample at flat index `i`, `w` the raw row stride,
`epsilon = 1e-5`, `epsilon2 = 1e-10`, and:

```text
lerp(a, v0, v1) = v0 + a * (v1 - v0)
```

The interior stencil needs four samples in every direction, so the RCD border is four
raw pixels.

#### Step R1 — copy native samples

Initialize R, G, and B to zero. At every raw coordinate, copy `C` into the output plane
selected by the Bayer pattern. These locations must never be overwritten.

#### Step R2 — vertical/horizontal discrimination

For every position with the needed support, compute the seven-tap high-pass responses:

```text
HPF_V(i) =
    C[i-3w] - C[i-w] - C[i+w] + C[i+3w]
    - 3 * (C[i-2w] + C[i+2w])
    + 6 * C[i]

HPF_H(i) =
    C[i-3] - C[i-1] - C[i+1] + C[i+3]
    - 3 * (C[i-2] + C[i+2])
    + 6 * C[i]

V(i) = max(epsilon2, HPF_V(i-w)^2 + HPF_V(i)^2 + HPF_V(i+w)^2)
H(i) = max(epsilon2, HPF_H(i-1)^2 + HPF_H(i)^2 + HPF_H(i+1)^2)
VH(i) = V(i) / (V(i) + H(i))
```

`VH = 0` favors vertical interpolation; `VH = 1` favors horizontal.

#### Step R3 — low-pass plane

Compute the unnormalized 3 × 3 low-pass value:

```text
L(i) = C[i]
     + 0.5  * (C[i-w] + C[i+w] + C[i-1] + C[i+1])
     + 0.25 * (C[i-w-1] + C[i-w+1] + C[i+w-1] + C[i+w+1])
```

Its DC gain is four. Do not divide by four; the ratio formula assumes this gain.

#### Step R4 — green at red/blue positions

At every R or B site, compute cardinal gradients:

```text
N = epsilon
  + abs(C[i-w] - C[i+w])
  + abs(C[i]   - C[i-2w])
  + abs(C[i-w] - C[i-3w])
  + abs(C[i-2w] - C[i-4w])

S = epsilon
  + abs(C[i-w] - C[i+w])
  + abs(C[i]   - C[i+2w])
  + abs(C[i+w] - C[i+3w])
  + abs(C[i+2w] - C[i+4w])
```

`W` and `E` are the exact 90-degree rotations.

For a neighboring green `g` and a same-color site two pixels away with low-pass `L2`,
the original positive-domain ratio estimate is:

```text
ratio_green(g, L0, L2) = 2 * g * L0 / (epsilon + L0 + L2)
```

Calibration produces signed samples, so denominator cancellation must be handled
continuously. Lumos uses:

```text
denom = epsilon + L0 + L2
numer = 2 * g * L0

if L0 >= 0 and L2 >= 0:
    estimate = numer / denom
else:
    scale = epsilon + abs(L0) + abs(L2)
    transition = 0.25 * scale

    if abs(denom) >= transition:
        estimate = numer / denom
    else:
        additive = g + (L0 - L2) / 8
        t = abs(denom) / transition
        curve = t * (3 - 2*t)
        ratio_weight = t * curve
        weighted_ratio = numer * sign(denom) * curve / transition
        estimate =
            additive * (1 - ratio_weight) + weighted_ratio
```

The additive term divides the low-pass difference by eight because same-color LPFs
are two pixels apart and `L` has gain four. This branch prevents blow-ups around
signed low-pass cancellation without imposing a positive clamp.

Then:

```text
Gn = signed_estimate(C[i-w], L[i], L[i-2w])
Gs = signed_estimate(C[i+w], L[i], L[i+2w])
Gw = signed_estimate(C[i-1], L[i], L[i-2])
Ge = signed_estimate(C[i+1], L[i], L[i+2])

Gv = (S*Gn + N*Gs) / (N+S)
Gh = (W*Ge + E*Gw) / (W+E)

neighbor_VH = mean(VH[i-w-1], VH[i-w+1], VH[i+w-1], VH[i+w+1])
disc = whichever of VH[i] and neighbor_VH is farther from 0.5
G[i] = lerp(disc, Gv, Gh)
```

The apparently crossed weights give more weight to the estimate from the smaller
opposite gradient.

#### Step R5 — diagonal discrimination

Define the two diagonals `P = NW-SE` and `Q = NE-SW`. Compute the same seven-tap
high-pass response along each:

```text
HPF_P(i) =
    C[i-3w-3] - C[i-w-1] - C[i+w+1] + C[i+3w+3]
    - 3 * (C[i-2w-2] + C[i+2w+2])
    + 6 * C[i]

HPF_Q(i) =
    C[i-3w+3] - C[i-w+1] - C[i+w-1] + C[i+3w-3]
    - 3 * (C[i-2w+2] + C[i+2w-2])
    + 6 * C[i]
```

Sum three adjacent squared responses along each diagonal, floor each sum by
`epsilon2`:

```text
P_stat(i) = max(epsilon2,
    HPF_P(i-w-1)^2 + HPF_P(i)^2 + HPF_P(i+w+1)^2)

Q_stat(i) = max(epsilon2,
    HPF_Q(i-w+1)^2 + HPF_Q(i)^2 + HPF_Q(i+w-1)^2)

PQ(i) = P_stat(i) / (P_stat(i) + Q_stat(i))
```

Refine `PQ` exactly like `VH`: compare the center to the mean of the four diagonal
`PQ` neighbors and use whichever is farther from 0.5.

#### Step R6 — opposite color at red/blue positions

At an R site reconstruct B; at a B site reconstruct R. Let `X` be that target color.
For each diagonal, use native target samples and already reconstructed green:

```text
dNW = X[i-w-1] - G[i-w-1]
dNE = X[i-w+1] - G[i-w+1]
dSW = X[i+w-1] - G[i+w-1]
dSE = X[i+w+1] - G[i+w+1]
```

The NW gradient is:

```text
gNW = epsilon
    + abs(X[i-w-1] - X[i+w+1])
    + abs(X[i-w-1] - X[i-3w-3])
    + abs(G[i] - G[i-2w-2])
```

Rotate it to obtain `gNE`, `gSW`, and `gSE`. Then:

```text
Dp = (gNW*dSE + gSE*dNW) / (gNW+gSE)
Dq = (gNE*dSW + gSW*dNE) / (gNE+gSW)
X[i] = G[i] + lerp(PQ_disc, Dp, Dq)
```

#### Step R7 — red and blue at green positions

For each target `X in {R, B}` at a green site, interpolate the color difference
`X-G` along cardinal directions. Define:

```text
baseN = epsilon + abs(G[i] - G[i-2w])
baseS = epsilon + abs(G[i] - G[i+2w])
baseW = epsilon + abs(G[i] - G[i-2])
baseE = epsilon + abs(G[i] - G[i+2])

gN = baseN
   + abs(X[i-w] - X[i+w])
   + abs(X[i-w] - X[i-3w])
```

and rotate for the other directions. Let `dN = X[i-w]-G[i-w]` and similarly for
`dS`, `dW`, `dE`:

```text
Dv = (gN*dS + gS*dN) / (gN+gS)
Dh = (gE*dW + gW*dE) / (gW+gE)
X[i] = G[i] + lerp(VH_disc, Dv, Dh)
```

#### Step R8 — border and crop

Within four raw pixels of an edge:

1. copy the native sample exactly into its native plane;
2. for each missing channel, average same-color CFA samples in the valid 3 × 3
   neighborhood;
3. if none exist, search the valid 5 × 5 neighborhood;
4. use zero only if the image is too small to contain that color at all.

Run the stencil in full-raw coordinates so margins can support the active boundary,
then copy the active rectangle to the output planes. No output clamp is part of RCD.

### 5.3 X-Trans: Markesteijn one-pass

Lumos uses the four-direction, one-pass Markesteijn algorithm. Two-pass Markesteijn
uses eight directions and a refinement stage; it is a different algorithm and must not
be claimed unless implemented and tested.

Let colors be `R=0`, `G=1`, `B=2`, `P = width*height`, and `NDIR=4`. Directional
green storage is `green[d*P + i]`. Directional red/blue candidates are stored beside
it. The informational border is eight active pixels.

#### Step X0 — validate and build hexagonal lookup

A valid 6 × 6 X-Trans period contains exactly 8 R, 20 G, and 8 B entries. For every
green position, validate the axial red/blue-neighbor geometry expected by the lookup.

Build the eight neighbor offsets with the canonical tables:

```text
orth = [1,0, 0,1, -1,0, 0,-1, 1,0, 0,1]

patt[non_green] =
    [0,1, 0,-1, 2,0, -1,0, 1,1, 1,-1, 0,0, 0,0]

patt[green] =
    [0,1, 0,-2, 1,0, -2,0, 1,1, -2,-2, 1,-1, -1,1]
```

For each pattern coordinate `(row,col)` in a 3 × 3 fundamental phase, follow the
canonical `orth` walk, track consecutive non-green axial neighbors, and identify the
solitary-green phase where all four axial neighbors are non-green. When the canonical
condition `ng == is_green + 1` is met, construct eight offsets:

```text
v = orth[d]   * patt[g][2*c] + orth[d+1] * patt[g][2*c+1]
h = orth[d+2] * patt[g][2*c] + orth[d+3] * patt[g][2*c+1]
hex[c xor ((g*2) & d)] = (v, h)
```

where `g` is 0 for a non-green center and 1 for green. Store lookup entries for every
periodic phase and use raw-coordinate modulo, including active margins.

#### Step X1 — allowed green range

At a native green site, `gmin = gmax = raw`. At every non-green site, inspect the
first six canonical hex neighbors and set:

```text
gmin = minimum(neighbor green samples)
gmax = maximum(neighbor green samples)
```

Failure to find a valid neighbor indicates an invalid pattern or insufficient padded
geometry; it must not silently substitute zero in the interior.

#### Step X2 — four green candidates

At native green sites, copy the raw value into all four directions. At a non-green
site with center sample `C` and hex neighbors `h0..h5`:

```text
A = 0.6796875 * (h0 + h1)
  - 0.1796875 * (sample(2*h0) + sample(2*h1))

B = 0.87109375 * h3
  + 0.12890625 * h2
  + 0.359375 * (C - sample(-h2))

C0 = 0.640625 * h4
   + 0.359375 * sample(-2*h4)
   + 0.12890625 * (2*C - sample(3*h4) - sample(-3*h4))

C1 = 0.640625 * h5
   + 0.359375 * sample(-2*h5)
   + 0.12890625 * (2*C - sample(3*h5) - sample(-3*h5))
```

The alternating X-Trans row phase swaps the canonical direction slots:

```text
flip = ((raw_y - solitary_green_row) mod 3 == 0) ? 1 : 0
direction = candidate_index xor flip
```

Clamp each green candidate to `[gmin, gmax]`. This is an interpolation bound inside
Markesteijn, not a global `[0,1]` clamp; signed minima and maxima remain signed.

#### Step X3 — reconstruct directional red and blue

This stage has three ordered geometry passes.

**A. Solitary green sites.** For horizontal and vertical candidate axes, and distances
one and two, reconstruct the appropriate target color:

```text
green_correction = 2*G0 - Gplus - Gminus
X = 0.5 * (green_correction + Xplus + Xminus)

inconsistency +=
    (Gplus - Gminus - Xplus + Xminus)^2
    + green_correction^2
```

At the two diagonal direction slots, evaluate both horizontal and vertical
constructions and retain the one with smaller total inconsistency. The target color
alternates between R and B with distance according to the validated pattern.

**B. Opposite color at native R/B sites.** Preserve the native color. Reconstruct the
opposite color from a symmetric pair:

```text
X0 = G0 + 0.5 * (Xplus + Xminus - Gplus - Gminus)
```

The primary axis is vertical except on the solitary-green row phase, where it is
horizontal. The alternate long axis is the perpendicular axis at distance three.
For primary and long axes:

```text
gradient(axis) = abs(G0-Gplus) + abs(G0-Gminus)
```

Select the axis with the exact phase rule:

```text
primary_vertical = ((raw_y - solitary_green_row) mod 3 != 0)
primary_parity = 0 if primary_vertical else 1

use_primary =
    direction > 1
    or ((direction xor primary_parity) & 1) != 0
    or primary_gradient < 2*long_gradient
```

Use the primary pair when `use_primary` is true and the long pair otherwise. Thus
directions 2 and 3 always use the primary axis; exactly one of directions 0 and 1 may
use the long axis, depending on phase and the gradient comparison.

**C. Two-by-two green blocks.** Only direction slots 0 and 1 are independently
reconstructed here. Take the direction's pair of canonical hex neighbors. For a
symmetric pair:

```text
X0 = (2*G0 - G1 - G2 + X1 + X2) / 2
```

For an asymmetric pair, weight the first neighbor two-to-one:

```text
X0 = (3*G0 - 2*G1 - G2 + 2*X1 + X2) / 3
```

The preceding passes guarantee that `X1` and `X2` are native or already reconstructed;
do not parallelize the three passes across one another.

#### Step X4 — directional derivatives

Materialize `(R,G,B)` for every direction and convert to the algorithm's YPbPr-like
space:

```text
Y  = 0.2627*R + 0.6780*G + 0.0593*B
Pb = 0.56433*(B-Y)
Pr = 0.67815*(R-Y)
```

Direction offsets are:

```text
d0 = (0, 1)
d1 = (1, 0)
d2 = (1, 1)
d3 = (1,-1)
```

For component `Q in {Y,Pb,Pr}`:

```text
lap_Q = 2*Q(center) - Q(center+d) - Q(center-d)
derivative[d] = lap_Y^2 + lap_Pb^2 + lap_Pr^2
```

#### Step X5 — homogeneity maps

At each pixel:

```text
threshold = 8 * min_d(derivative[d])
```

For every direction, count in the 3 × 3 neighborhood how many positions satisfy:

```text
derivative[d][neighbor] <= threshold[center]
```

This yields a `u8` homogeneity count from zero through nine for each direction.
Notice that the center's threshold is used for all nine comparisons.

#### Step X6 — select and blend directions

For each direction, sum its homogeneity counts in the centered 5 × 5 neighborhood.
Near an edge, use the in-bounds intersection. Let:

```text
best = max(score[0..4])
accept_threshold = best - floor(best/8)
accepted = all d where score[d] >= accept_threshold
```

Average R, G, and B candidates over every accepted direction with equal weight. At
least one direction is accepted because one attained `best`.

#### Step X7 — border

Replace the outer eight active pixels with local same-color interpolation:

- preserve the native channel exactly;
- inspect the in-bounds 3 × 3 neighborhood;
- weight axial neighbors by `0.5` and diagonal neighbors by `0.25`;
- divide each missing channel by its accumulated weight.

If no sample of a missing color exists, a small-image fallback must be explicit.
Lumos's present fallback copies the center raw sample into all channels in one
green/no-red case and otherwise uses the center when a channel has no weight. Tests
must lock this behavior down or replace it with a well-defined wider search.

No final clipping belongs to Markesteijn.

### 5.4 Optional lower-cost methods

For previews or diagnostics, bilinear, super-pixel, or Malvar-He-Cutler may be useful,
but each needs an explicit API choice:

- **bilinear** averages same-color neighbors and is fast but produces zippering and
  color artifacts;
- **super-pixel** maps each Bayer 2 × 2 cell to one RGB pixel, combining the two greens;
  it avoids color interpolation but halves width and height and needs a policy for the
  two green samples;
- **Malvar-He-Cutler** uses fixed 5 × 5 linear filters with Laplacian cross-channel
  corrections and is a good deterministic reference;
- **CFA drizzle** maps individual CFA photosites directly into per-color output planes
  during registration and can avoid demosaic before integration.

Do not label one of these `RCD` or `Markesteijn`. If implemented, derive every kernel
and border rule from the cited primary/reference implementation and add exact fixtures;
a single example kernel is not a complete Malvar implementation.

## 6. PNG, TIFF, and JPEG

Generic raster formats are not automatically scientific linear data.

### 6.1 Transfer functions

For a normalized sRGB encoded value `E`, inverse transfer is:

```text
linear(E) =
    E / 12.92                         if E <= 0.04045
    ((E + 0.055) / 1.055) ^ 2.4      otherwise
```

Apply this only when the file is actually tagged or explicitly assumed sRGB. PNG may
carry `iCCP`, `cICP`, `sRGB`, or `gAMA`/`cHRM` information with defined precedence.
TIFF can carry ICC profiles, transfer functions, sample formats, and CFA tags. JPEG is
normally display-referred and lossy.

The generic decoder must not simply cast integer code values to `f32` and call the
result linear. A correct color path either:

- color-manages the image into a declared linear working space; or
- rejects it for scientific processing and marks it preview-only.

### 6.2 Integer normalization

After decoding the file's exact integer sample type:

```text
encoded_unit = code / maximum_code_for_declared_bit_depth
linear = inverse_declared_transfer(encoded_unit)
```

Do not normalize a 12-bit value stored in a 16-bit container by 65535 unless the format
declares that the full 16-bit range is meaningful. Preserve bit-depth/sample-format
metadata.

### 6.3 Alpha and lossy input

Alpha requires an explicit policy:

- reject alpha for scientific input;
- unpremultiply in linear light when the encoding declares premultiplied data;
- composite in linear light over a declared background;
- or retain a validity/coverage mask.

Silently dropping alpha can turn transparent garbage RGB into valid pixels.

JPEG, tone-mapped PNG, and ordinary display TIFF should be preview/reference inputs,
not calibration frames. Their quantization, transfer encoding, color transforms, and
possible clipping violate the sensor-domain contract.

## 7. Metadata, provenance, and errors

### 7.1 Required compatibility metadata

Calibration-set validation needs, where available:

- dimensions, active area, orientation, and binning;
- mono/Bayer/X-Trans layout and exact phase;
- camera/detector identity;
- exposure time and frame type;
- ISO or analog gain, electronic gain, and offset;
- detector temperature and set point;
- filter;
- black model and normalization denominator;
- white/saturation level and validity masks;
- whether linearization, clipping, white balance, color conversion, or demosaic ran.

Missing metadata is not always fatal, but it must remain `Unknown` rather than being
filled with plausible defaults. Compatibility code can then ask for a profile or reject
the set.

### 7.2 Provenance record

Each loaded frame should carry a compact transform record:

```text
container and decoder version
selected HDU or RAW image
stored/logical sample type
physical scaling or sensor normalization
orientation transforms
CFA phase transform
linearization status
invalid/saturation policy
demosaic algorithm and version
post-decode color/transfer operations
```

This record makes cached intermediates reproducible and prevents a preview from being
mistaken for a calibration input.

### 7.3 Diagnostics

Errors should include the path and the structural context needed to act:

- unsupported signature/extension;
- HDU index/name and offending keyword;
- expected and actual shape/sample count;
- first invalid coordinate and total invalid count;
- RAW make/model and unsupported sensor layout;
- conflicting CFA metadata;
- black/white metadata values that violate invariants;
- cancellation as a distinct non-corruption outcome.

Do not silently fall back across scientific-domain boundaries. For example, failure to
obtain raw CFA data must not quietly return a developed RGB image to the calibration
pipeline.

## 8. Performance and memory

Correctness comes first, but the stage is bandwidth-heavy.

- Parse headers before allocating image-sized buffers.
- Decode directly into the final stored type where the library API permits.
- Evaluate FITS scaling in `f64` and narrow once.
- Keep RGB planar to match downstream Lumos buffers.
- Reuse full-image scratch planes only after the previous stage is complete.
- Parallelize independent rows/tiles; preserve stage barriers where a pass reads the
  completed result of another pass.
- Check cancellation between passes and at bounded tile intervals.
- Bound decompression output by validated logical dimensions to prevent compression
  bombs.
- Include decoder buffers, masks, and demosaic scratch in memory-budget estimates.

For an active image of `P` pixels, current RCD needs the CFA plus several full-size
`f32` planes and two half-width diagonal buffers; current one-pass Markesteijn uses
four directional candidate sets plus derivative/homogeneity scratch. The implementation
should expose a conservative peak-memory estimator before launching work.

## 9. Required implementation sequence

The preferred order for completing Stage 1 is:

1. Split the public API into `CfaImage`, `LinearImage`, and `PreviewImage` constructors.
2. Replace extension-only dispatch with content-confirmed detection.
3. Correct FITS numeric provenance and remove `DATAMAX` normalization.
4. Add explicit FITS HDU selection, checksum policy, and metadata inheritance policy.
5. Implement orientation as a pixel-coordinate transform with height-aware CFA phase.
6. Route mosaic FITS into `CfaImage` and the same calibrate-then-demosaic pipeline as
   RAW.
7. Expand RAW metadata and add invalid/saturation masks.
8. Prove DNG linearization behavior with fixtures.
9. Add explicit transfer/color management for standard images or restrict them to
   preview use.
10. Add lower-cost demosaic/CFA-drizzle options only after the scientific path is
    invariant-tested.

## 10. Current Lumos state and gap audit

This section describes the repository at the time of this document update. The
preceding sections are the required behavior.

### 10.1 Already implemented well

- `fits-well` scans image-bearing HDUs and transparently reads plain and tile-compressed
  images.
- `physical_f32()` applies `BSCALE`/`BZERO` in `f64`, turns `BLANK` into NaN, and
  narrows to `f32` with defined rounding.
- Lumos preserves those FITS physical values unchanged; `DATAMAX` is metadata only.
- FITS loading selects an explicit HDU by zero-based index or `EXTNAME`/`EXTVER`; the
  default accepts only an unambiguous single image HDU.
- Two-dimensional mono and one-plane FITS shapes are accepted directly. Three-plane
  cubes require explicit RGB interpretation before conversion to planar storage.
- Selected-HDU provenance records index/name/version and `DATASUM`/`CHECKSUM` status;
  absent checksums are accepted by default, while invalid checksums are rejected.
- FITS shape and source/output/peak byte budgets are checked before pixel decode.
  Plain and compressed images are converted in bounded row sections with cancellation
  polls between chunks.
- RAW loading keeps full dimensions and active margins separate.
- The LibRaw black model is consolidated into common, per-channel, and repeating
  residual components.
- `load_raw_cfa` preserves signed normalized samples for calibration.
- `PreviewImage::from_file`, `LinearImage::from_file`, and `CfaImage::from_file` establish
  separate preview, linear-scientific, and sensor-CFA products with decoder, transfer,
  color, clipping, and demosaic provenance.
- Path stacking admits only non-mosaic physical FITS and explicitly declared float
  TIFF; PNG, JPEG, integer/alpha TIFF, direct RAW preview, and mosaic FITS are rejected.
- Camera RAW and eligible one-plane FITS loaded through `CfaImage::from_file` share the
  same calibration, optional cosmic-ray, and demosaic path.
- Bayer RCD has a signed-denominator fallback instead of forcing calibrated values
  positive.
- X-Trans uses a native one-pass Markesteijn implementation and validates its pattern.
- Direct LibRaw fallback explicitly requests unity multipliers, linear gamma, no
  auto-brightness, and no output color conversion.

### 10.2 Correctness gaps

1. **Logical FITS types are folded incorrectly.** `I8` is reported as `UInt8` and
   `U64` as `Int64` in descriptive metadata. Large integer and `F64` physical samples
   also narrow to `f32` without a caller-visible precision policy or provenance flag.
2. **Dispatch is extension-only.** Common `.fts`, `.fits.fz`, and outer-gzip
   `.fits.gz` names are rejected (the compound names are examined only by their final
   extension), and the hard-coded RAW list exposes only
   RAF/CR2/CR3/NEF/ARW/DNG despite broader LibRaw support.
3. **FITS metadata policy is incomplete.** HDU index or `EXTNAME`/`EXTVER` selection,
   explicit RGB cube semantics, and checksum verification are implemented. `INHERIT`
   policy, compressed-float quantization provenance, and complete WCS retention remain
   missing.
4. **FITS CFA support is incomplete.** `BOTTOM-UP` unconditionally calls
   `flip_vertical()`; it should shift vertical phase by `(height-1) mod 2` if rows are
   reversed. `COLORTYP` conflicts and 36-character X-Trans FITS patterns are not
   handled.
5. **RAW compatibility metadata is sparse.** The current output does not retain enough
   exposure, temperature, gain/offset, white-level, camera identity, linearization,
   or mask information for robust calibration-set validation.
6. **No saturation/validity masks.** FITS nulls currently reject the complete image,
   while RAW saturation and bad-pixel states are not propagated.
7. **Unknown-CFA fallback is mislabeled.** A developed RGB LibRaw fallback can carry
   `CfaType::Mono` metadata. Scientific loading correctly errors instead; preview
   metadata still needs correction.
8. **Preview rasters are not color-managed.** PNG/JPEG/TIFF preview samples are
   converted to `f32` without interpreting transfer or ICC metadata. RGBA alpha is
   dropped, although that decision is now recorded. Scientific loading rejects these
   inputs except for explicitly declared non-alpha float TIFF.
9. **DNG conformance is unproven and incomplete.** LibRaw parses
    `LinearizationTable` and its DNG unpackers apply the curve, but Lumos has no
    fixtures demonstrating exactly-once behavior for its pinned version and supported
    compression variants. LibRaw 0.20.1 also reduces `BlackLevelDeltaH` and
    `BlackLevelDeltaV` to mean offsets, and Lumos uses `maximum-common_black` rather
    than DNG's `WhiteLevel-maximum_black` normalization. `raw_image_slice` accepts
    only LibRaw's one-component `u16 raw_image`, so floating-point and multi-component
    DNG raw buffers are unsupported rather than handled through `float_image`,
    `color3_image`, or `color4_image`.
10. **No CFA drizzle or super-pixel path.** These are useful optional integration and
    preview modes, but lower priority than the domain and FITS-CFA fixes above.

### 10.3 Relevant source locations

- `lumos/src/io/image/mod.rs` — shared metadata, preview dispatch, and standard preview conversion.
- `lumos/src/io/image/linear.rs` — linear image representation, scientific loading, and layout conversion.
- `lumos/src/io/image/fits/mod.rs` — FITS selection, scaling, metadata, and Bayer
  interpretation.
- `lumos/src/io/image/cfa.rs` — signed CFA representation and demosaic routing.
- `lumos/src/io/raw/mod.rs` — LibRaw boundary, black consolidation, RAW products.
- `lumos/src/io/raw/normalize.rs` — clipped and unclipped normalization.
- `lumos/src/io/raw/demosaic/bayer/rcd.rs` — signed RCD.
- `lumos/src/io/raw/demosaic/xtrans/` — X-Trans validation, lookup, and one-pass
  Markesteijn.
- `lumos/src/stacking/pipeline/streaming.rs` — scientific frame order.
- `fits-well/src/reader/mod.rs` — HDU scanning/selection and checksum verification.
- `fits-well/src/data/mod.rs` — logical FITS sample types and physical conversion.
- `fits-well/src/compress/` — tiled-image compression and decompression.

## 11. Verification specification

Tests must assert exact values or exact masks, not merely that decoding succeeds.

### 11.1 FITS fixtures

Generate small hand-computable files for:

- every legal `BITPIX`;
- canonical signed-byte and unsigned 16/32/64 `BZERO` cases;
- nontrivial positive and negative `BSCALE`/`BZERO`;
- `BLANK` before scaling and floating NaN;
- big-endian values whose byte reversal is obvious;
- primary image, empty primary plus IMAGE extension, and multiple selectable images;
- tiled Rice/GZIP/PLIO/HCOMPRESS examples supported by `fits-well`;
- lossless integer compression and quantized floating compression, including the
  declared dithering method and quantization error policy;
- valid, absent, and corrupt `CHECKSUM`/`DATASUM`;
- mono, `NAXIS3=1`, RGB, and rejected cubes;
- huge/overflowing dimensions without allocation;
- large `I64`/`U64` precision-policy boundaries.

For the `DATAMAX` regression, make two otherwise identical integer images with
different `DATAMAX` cards and assert every decoded physical sample is identical.

Cross-check physical arrays against CFITSIO or Astropy for the same files, while
keeping Lumos's shape and invalid-sample policy explicit.

### 11.2 FITS CFA fixtures

For each Bayer pattern, sweep:

- odd and even widths;
- odd and even heights;
- `TOP-DOWN` and `BOTTOM-UP`;
- even/odd `XBAYROFF` and `YBAYROFF`;
- absent `ROWORDER` with each configured fallback.

Fill each photosite with a distinct value by physical sensor color, transform it into
the file orientation, load it, and assert `color_at(x,y)` matches every sample. Include
Siril's public orientation test corpus as interoperability fixtures.

### 11.3 RAW normalization fixtures

Use synthetic sensor buffers with hand-computed expectations:

- common black only;
- four different Bayer channel blacks;
- 2 × 2 and larger repeat tables;
- active margins that shift both CFA and repeat phase;
- raw values below black, at black, at white, and above white;
- white equal to or below common black as rejection cases;
- a non-identity DNG linearization table;
- nonconstant DNG `BlackLevelDeltaH` and `BlackLevelDeltaV` vectors;
- saturation and invalid masks.

For example, with `B=64`, `C[R]=4`, `T[phase]=2`, and
`white_reference=1088`:

```text
D = 1088 - 64 = 1024
raw = 582
sample = (582 - 64 - 4 - 2) / 1024 = 0.5
quantization_sigma = 1 / (1024 * sqrt(12))
```

Assert the scientific result is exactly `0.5` within the single expected `f32` rounding
and that a raw value below 70 remains negative. Assert the preview path clips that same
negative value and is marked non-scientific.

### 11.4 Demosaic invariants

For both algorithms:

- native samples equal their source CFA samples exactly;
- a constant mosaic remains constant in all three channels, including borders;
- negative constants and sign-changing ramps remain finite;
- no input or output is implicitly clamped;
- translating/cropping by a pattern period preserves the interior result;
- cancellation returns no partial image;
- tiny supported dimensions use the documented fallback; smaller dimensions reject.

For RCD:

- test all four Bayer patterns and odd/even margins;
- hand-check HPF, low-pass, gradient, and blend stages on a synthetic patch;
- construct `L0 + L2 ~= -epsilon` and assert the signed fallback is finite and
  continuous on both sides;
- compare positive-domain interiors with the `librtprocess` scalar reference;
- lock the four-pixel border policy.

For Markesteijn:

- validate the 8/20/8 pattern counts and reject malformed patterns;
- assert the canonical hex offsets for every periodic phase;
- hand-check `A`, `B`, `C0`, and `C1` candidates;
- assert changing a directional edge changes the selected homogeneity directions;
- compare interior candidates and final output with one-pass `librtprocess`;
- lock the eight-pixel border policy.

Use real-camera crops only after these synthetic tests. Store expected hashes or exact
reference arrays with decoder/library version provenance; a visually pleasing render
is not a correctness oracle.

### 11.5 Standard-image fixtures

- exact inverse-sRGB values below, at, and above `0.04045`;
- linear PNG with explicit metadata;
- PNG with conflicting color chunks, following PNG precedence;
- 8- and 16-bit samples;
- RGBA with each allowed alpha policy;
- JPEG and untagged TIFF rejected for scientific CFA intent;
- a tagged linear TIFF accepted only through the dedicated policy.

## 12. Primary and implementation references

### Standards and format specifications

- [FITS Standard 4.0](https://fits.gsfc.nasa.gov/standard40/fits_standard40aa-le.pdf)
  — HDUs, image data, numeric representation, scaling, nulls, keywords, checksums, and
  tiled-image compression.
- [Official FITS standard page](https://fits.gsfc.nasa.gov/fits_standard.html) —
  current approved version and related conventions.
- [FITS user's guide: reserved keywords](https://fits.gsfc.nasa.gov/users_guide/users_guide/node22.html)
  — concise `BSCALE`, `BZERO`, `BLANK`, `DATAMIN`, and `DATAMAX` semantics.
- [FITS user's guide: image orientation](https://fits.gsfc.nasa.gov/users_guide/users_guide/node52.html)
  — historic lower-left recommendation and its status.
- [Digital Negative Specification 1.7.1.0](https://helpx.adobe.com/content/dam/help/en/camera-raw/digital-negative/jcr_content/root/content/flex/items/position/position-par/download_section_733958301/download-1/DNG_Spec_1_7_1_0.pdf)
  and [DNG SDK downloads](https://helpx.adobe.com/camera-raw/digital-negative.html) —
  linearization, black subtraction, normalization, clipping, CFA metadata, and the
  reference SDK.
- [PNG Third Edition](https://www.w3.org/TR/png-3/) — PNG sample, alpha, and color
  metadata rules.
- [IEC sRGB transfer-function summary](https://www.w3.org/Graphics/Color/srgb) —
  standard sRGB encoding and inverse-transform constants.

### Open-source implementations and interoperability

- [fits-well 0.1.5 documentation](https://docs.rs/fits-well/0.1.5/fits_well/) and
  [repository](https://github.com/xorza/fits) — Lumos's in-tree pure-Rust FITS parser,
  plain/compressed image reader, logical integer views, and physical conversion.
- [CFITSIO](https://heasarc.gsfc.nasa.gov/fitsio/) — mature reference FITS
  implementation and interoperability oracle.
- [LibRaw API overview](https://www.libraw.org/docs/API-overview.html) and
  [data/output parameters](https://www.libraw.org/docs/API-datastruct.html) — RAW
  unpacking and processing contracts.
- [LibRaw source](https://github.com/LibRaw/LibRaw) — especially
  `adjust_bl`, `subtract_black_internal`, DNG parsing/unpackers, and color scaling.
- [LibRaw discussion of unprocessed output](https://www.libraw.org/node/2251) —
  why `output_color=0` alone does not disable white balance/brightness behavior.
- [librtprocess](https://github.com/CarVac/librtprocess) — `rcd.cc`,
  `markesteijn.cc`, X-Trans validation, and border reference implementations.
- [Original RCD implementation](https://github.com/LuisSR/RCD-Demosaicing) — Luis
  Sanz Rodríguez's Ratio Corrected Demosaicing.
- [Siril source](https://gitlab.com/free-astro/siril) — RAW/FITS conversion,
  `librtprocess` integration, CFA handling, and astronomy workflow comparison.
- [Siril FITS orientation convention](https://free-astro.org/index.php?title=Siril%3AFITS_orientation%2Fen)
  — `ROWORDER`, Bayer offsets, and public orientation fixtures.
- [Siril documentation](https://siril.readthedocs.io/en/latest/) — calibration,
  demosaic, sequence conversion, and CFA drizzle workflows.
- [RawPedia demosaicing guide](https://rawpedia.rawtherapee.com/Demosaicing) —
  practical RCD/AMaZE/Markesteijn behavior and artifact tradeoffs.
- [darktable demosaic documentation](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/demosaic/)
  — production Bayer/X-Trans algorithm selection and passthrough modes.
- [Malvar-He-Cutler IPOL article and source](https://www.ipol.im/pub/art/2011/g_mhcd/)
  — reproducible fixed-kernel baseline, paper, and C implementation.

When a formula in this document and a dependency disagree, first check the version
actually pinned by `Cargo.lock`. Update the implementation, fixtures, and this document
together; do not silently adopt behavior from a newer upstream release.
