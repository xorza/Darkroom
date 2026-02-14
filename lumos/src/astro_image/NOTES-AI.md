# astro_image Module

## Overview

Core data structure (`AstroImage`) for astronomical image processing. Planar f32 pixel
storage, FITS/RAW/standard image I/O, CFA handling, calibration operations.

Files: `mod.rs` (AstroImage, PixelData, ImageDimensions, conversions), `fits.rs` (FITS
loading via cfitsio/fitsio-rs), `sensor.rs` (libraw sensor detection), `cfa.rs` (CfaImage
for raw CFA data, calibration ops, demosaic dispatch).

## Architecture

- **PixelData**: Enum of `L(Buffer2<f32>)` or `Rgb([Buffer2<f32>; 3])` -- planar storage
- **AstroImage**: PixelData + ImageDimensions + AstroImageMetadata
- **CfaImage**: Single-channel raw sensor data with CFA pattern metadata
- **ImageDimensions**: width/height/channels (1 or 3 only)
- **BitPix**: Enum with UInt8/Int16/UInt16/Int32/UInt32/Int64/Float32/Float64 variants
  - `from_fits_value()` returns `Result<Self, String>` for safe external input handling
  - `normalization_max()` returns divisor for [0,1] normalization (None for floats)
  - `to_fits_value()` maps back to FITS BITPIX integer (unsigned maps to signed equivalent)
- FITS loading via `fitsio` crate (thin wrapper around cfitsio C library)
- All pixel data stored as f32 normalized to [0,1] (both RAW and FITS)

## Current FITS Metadata Support

Reads these FITS keywords:
- OBJECT, INSTRUME, TELESCOP, DATE-OBS, EXPTIME
- FILTER, GAIN, EGAIN, CCD-TEMP (fallback: CCDTEMP)
- IMAGETYP (fallback: FRAME), XBINNING, YBINNING
- SET-TEMP, OFFSET, FOCALLEN, AIRMASS
- BAYERPAT, ROWORDER, XBAYROFF, YBAYROFF (for CFA detection)

## What It Does Well

- Planar storage matches PixInsight XISF convention for per-channel processing
- Type-safe PixelData enum prevents channel count mismatches
- Correct Rec. 709 luminance coefficients (0.2126, 0.7152, 0.0722)
- Proper sensor detection from libraw filters field (Bayer, X-Trans, Mono)
- Standard flat normalization formula: `light /= (flat - bias) / mean(flat - bias)`
- Rayon parallelism throughout (row-based chunking for cache locality)
- Good test coverage (~50 tests across module)
- Trait-based `StackableImage` abstraction for both AstroImage and CfaImage
- 3-channel FITS loaded correctly as planar data (via `from_planar_channels`)
- FITS integer data normalized to [0,1] on load (consistent with RAW normalization)
- BitPix distinguishes unsigned from signed types for correct normalization ranges
- X-Trans demosaic uses `process_xtrans_f32()` -- no f32-to-u16 precision loss
- `ImageDimensions::sample_count()` (w*h*c) vs `pixel_count()` (w*h) -- clear naming
- BAYERPAT/ROWORDER/XBAYROFF/YBAYROFF FITS header parsing for CFA pattern detection
- CfaPattern: `from_bayerpat()`, `flip_vertical()`, `flip_horizontal()` for FITS CFA
- Comprehensive FITS metadata including FILTER, GAIN, EGAIN, CCD-TEMP, IMAGETYP,
  XBINNING, YBINNING, SET-TEMP, OFFSET, FOCALLEN, AIRMASS (with fallbacks)

---

## Comparison with Industry Standards

### FITS Standard 4.0 Compliance

**BZERO/BSCALE Handling -- Correct.**
cfitsio automatically applies BZERO/BSCALE when reading images. For BITPIX=16 +
BZERO=32768 (unsigned 16-bit convention), cfitsio returns values in the 0-65535 range
as f32. The loader then normalizes to [0,1] by dividing by `BitPix::normalization_max()`.
The `image_type_to_bitpix()` function correctly maps cfitsio's `ImageType` enum
(which includes `UnsignedShort`, `UnsignedLong`, etc.) to our `BitPix` variants,
preserving the unsigned/signed distinction for correct normalization.

**3D FITS Data Order -- Correct.**
FITS uses Fortran-style (column-major) storage. For NAXIS1=W, NAXIS2=H, NAXIS3=3:
data is stored as W*H red values, then W*H green, then W*H blue. The fitsio-rs crate
reports shape in C order: [3, H, W]. The loader uses `from_planar_channels()` for
3-channel FITS to correctly handle this planar layout.

**Integer Data Convention -- Correct.**
FITS standard (4.0) only supports signed integers natively. Unsigned integers use the
BZERO convention: BITPIX=16 + BZERO=32768 + BSCALE=1 for unsigned 16-bit. This is by
far the most common format from astronomical cameras. cfitsio handles this transparently.

### Normalization: Comparison with Professional Tools

**PixInsight:**
- Works in normalized [0,1] floating point internally. Float FITS loaded assuming [0,1]
  range by default (configurable). Integer FITS normalized via BZERO/BSCALE then to [0,1].

**Siril:**
- Native formats: USHORT (16-bit) or FLOAT normalized to [0,1].
- For BITPIX -32/-64 FITS not already in [0,1]: divides by 65535 as heuristic.
- Signed SHORT converted to unsigned SHORT with BZERO offset.
- BITPIX 32 (int32): normalizes using (value - min) / (max - min).
- BITPIX 8: converted to 16-bit.

**Our implementation:**
- Integer types: divide by type maximum (UInt16 -> /65535, Int16 -> /32767, etc.)
- Float types: passed through unchanged (assumed already in correct range).

### BAYERPAT / CFA Convention

**Quasi-standard (SBIG SBFITSEXT).**
BAYERPAT, XBAYROFF, YBAYROFF originated from SBIG's FITS extensions (2003), adopted
by Software Bisque (CCDSoft) and Diffraction Limited (MaximDL). Not part of official
FITS standard but universally used by amateur astro cameras and capture software.

**Our handling -- Correct and thorough.**
Supports BAYERPAT values: RGGB, BGGR, GRBG, GBRG, TRUE (=RGGB). Adjusts for
ROWORDER (BOTTOM-UP flips pattern vertically). Handles XBAYROFF/YBAYROFF integer
offsets. This matches how NINA, MaximDL, Siril, and PixInsight interpret these keywords.

### WCS (World Coordinate System) Support

**Status: Not implemented in astro_image.**

WCS is implemented separately in the `registration/distortion/sip` module (SIP polynomial
distortion). The `astro_image` module does not read or store WCS keywords from FITS
headers. Key WCS keywords that professional tools read:

- **CRPIX1/CRPIX2**: Reference pixel coordinates
- **CRVAL1/CRVAL2**: RA/Dec at reference pixel (degrees)
- **CD1_1/CD1_2/CD2_1/CD2_2**: Rotation+scale matrix (modern standard)
- **CDELT1/CDELT2 + CROTA2**: Deprecated but still common
- **CTYPE1/CTYPE2**: Projection type (e.g. 'RA---TAN', 'DEC--TAN')
- **SIP coefficients**: A_p_q, B_p_q for distortion (A_ORDER, B_ORDER)

WCS is essential for plate-solved images from Astrometry.net, ASTAP, PinPoint.
The project has SIP polynomial support in the registration pipeline, so WCS parsing
may not be needed in the image loader itself -- depends on workflow.

---

## Issues Found

### Issue 1: Int16 Normalization Produces Wrong Values for Calibrated Data (Bug)

**File:** `mod.rs` line 67, `fits.rs` line 121-127

**Problem:** `Int16.normalization_max()` returns 32767.0. This means signed 16-bit FITS
values are divided by 32767, mapping the range [-32768, 32767] to approximately [-1, 1].
However, in practice, truly signed 16-bit FITS images (BITPIX=16, BZERO=0) from calibration
pipelines (e.g., dark-subtracted frames from MaximDL, Siril) may contain negative values
that represent real signal (overscan, noise floor). Dividing by 32767 maps negative values
to negative floats -- this is actually correct behavior for calibrated data.

**However**, the bigger issue is that cfitsio reports `ImageType::Short` for BITPIX=16
with BZERO=0 (truly signed), and `ImageType::UnsignedShort` for BITPIX=16 with
BZERO=32768 (unsigned convention). The `image_type_to_bitpix` function correctly
distinguishes these. But the normalization by 32767 for signed Int16 is asymmetric:
-32768 maps to -1.000031, while 32767 maps to exactly 1.0. This asymmetry is minor
(1 part in 32768) and acceptable for astronomical use.

**Verdict:** The current approach is reasonable. The Int16 path is rare -- nearly all
astronomical cameras use the unsigned convention (BZERO=32768), which correctly maps
to `UInt16` and normalizes by 65535. Truly signed Int16 FITS data is uncommon.

### Issue 2: Float FITS Data Not Validated or Normalized (Potential Bug)

**File:** `fits.rs` line 121-127

**Problem:** Float FITS data (BITPIX -32/-64) is passed through unchanged, assuming it is
already in [0,1] range. The FITS standard does NOT define a normalization convention for
float data. In practice:

- **PixInsight** defaults to [0,1] but allows any range.
- **Siril** normalizes float data not in [0,1] by dividing by 65535.
- **DeepSkyStacker** outputs float FITS in [0,65535] range.
- **Some scientific FITS** contain arbitrary physical values (flux, counts, etc.).

If a float FITS file from DeepSkyStacker (range 0-65535) or a scientific instrument
(arbitrary range) is loaded, pixel values will be wildly out of [0,1], causing incorrect
behavior in all downstream processing that assumes normalized data.

**Recommendation:** Add a heuristic: if float data max > 2.0, detect likely range and
normalize. Siril's approach (divide by 65535 if max > 1) is simple but effective. Better:
check DATAMAX/DATAMIN keywords if present, or compute actual min/max and decide.

### Issue 3: No FITS Writing Support

**File:** `mod.rs`

`save()` only supports PNG/JPEG/TIFF via the `imaginarium` crate. FITS is the primary
interchange format for astrophotography. ~~`save()` cloned the entire image~~ -- FIXED:
`to_image(&self)` borrows pixel data directly, no clone.

### Issue 4: Missing Metadata Keywords Compared to Industry Tools

**Files:** `fits.rs`, `mod.rs` (AstroImageMetadata)

Keywords present in NINA, MaximDL, Siril, and PixInsight but missing from our metadata:

**Coordinates / Location:**
- RA, DEC (telescope pointing, in degrees)
- OBJCTRA, OBJCTDEC (target coordinates in HH MM SS / DD MM SS format)
- SITELAT, SITELONG, SITEELEV (observatory location)

**Sensor / Optics:**
- XPIXSZ, YPIXSZ (pixel size in microns -- needed for plate scale calculation)
- FOCRATIO (focal ratio)
- ISOSPEED (DSLR ISO setting, distinct from GAIN)
- READOUTM (camera readout mode)
- DARKTIME (dark current integration time, may differ from EXPTIME)
- PEDESTAL (ADU offset added during calibration)
- DATAMAX (saturation threshold)

**Calibration State:**
- CALSTAT (calibration status: B=bias, D=dark, F=flat, BDF=all three)

**Timing:**
- DATE-LOC (local time at exposure start)

**Capture Software:**
- SWCREATE (software that created the file)
- OBSERVER (observer name)

**Priority ranking:**
- High: RA/DEC, XPIXSZ/YPIXSZ, READOUTM, DATAMAX, PEDESTAL
- Medium: OBJCTRA/OBJCTDEC, SITELAT/SITELONG, FOCRATIO, DARKTIME, CALSTAT
- Low: ISOSPEED, OBSERVER, SWCREATE, SITEELEV, DATE-LOC

### Issue 5: ROWORDER Not Applied to Image Data

**File:** `fits.rs` line 142-146

The ROWORDER keyword is used to adjust the Bayer pattern but the image data itself is
not flipped when ROWORDER=BOTTOM-UP. Professional tools handle this differently:

- **Siril**: Always reads/displays bottom-up; uses ROWORDER only for Bayer correction.
- **MaximDL**: Always writes TOP-DOWN with ROWORDER=TOP-DOWN.
- **FITS standard**: Recommends first pixel be lower-left (bottom-up).

Our behavior matches Siril's approach: ROWORDER affects only Bayer pattern, not pixel
layout. This is the pragmatic choice since most capture software writes TOP-DOWN despite
the FITS standard recommending BOTTOM-UP. Flipping image data would break alignment
with other software. **No action needed** -- current behavior is correct.

### Issue 6: No Multi-HDU Support

**File:** `fits.rs` line 15

Only the primary HDU is read. Some FITS files contain multiple image extensions (e.g.,
compressed FITS with image in extension 1, or multi-extension FITS from observatories).
cfitsio and the fitsio-rs crate support reading named/numbered HDUs. Low priority for
amateur astrophotography but relevant for compatibility with scientific FITS data.

### Issue 7: No FITS Compression Support

**File:** `fits.rs`

The loader does not explicitly handle or offer FITS tile compression (Rice, GZIP, GZIP-2).
cfitsio handles transparent decompression of compressed FITS files, so reading works.
However, there is no support for writing compressed FITS (relevant when Issue 3 is addressed).

### Issue 8: ISO Not Populated from FITS

**File:** `fits.rs` line 68

The `iso` field is always `None` for FITS files. The ISOSPEED keyword is used by NINA
and some DSLR capture software. Should try reading ISOSPEED from FITS headers.

---

## Memory Layout

**Planar storage** (`PixelData::Rgb([Buffer2<f32>; 3])`):
- Each channel is a contiguous `Vec<f32>` of width*height elements.
- Row-major order within each channel.
- Matches PixInsight XISF internal format.
- Optimal for per-channel operations (statistics, convolution, stacking).
- Suboptimal for per-pixel RGB operations (cache misses across 3 buffers).

**Buffer2<T>**: Simple wrapper around `Vec<T>` with width/height metadata.

**Trade-off:** Planar is the right choice for astro processing. Per-channel operations
dominate (background estimation, flat division, stacking, star detection). The interleave
cost when converting to display format (Image) is minimal compared to processing time.

## References

- FITS Standard 4.0: https://fits.gsfc.nasa.gov/standard40/fits_standard40aa.pdf
- FITS Dictionary: https://heasarc.gsfc.nasa.gov/docs/fcg/standard_dict.html
- cfitsio Unsigned Int Support: https://heasarc.gsfc.nasa.gov/docs/software/fitsio/c/c_user/node23.html
- cfitsio Data Scaling: https://heasarc.gsfc.nasa.gov/fitsio/c/c_user/node26.html
- MaxIm DL FITS Keywords: https://cdn.diffractionlimited.com/help/maximdl/FITS_File_Header_Definitions.htm
- N.I.N.A. FITS Keywords: https://nighttime-imaging.eu/docs/master/site/advanced/file_formats/fits/
- Siril FITS Format: https://siril.readthedocs.io/en/latest/file-formats/FITS.html
- Siril Supported FITS: https://free-astro.org/index.php?title=Siril:supported_FITS
- PixInsight XISF Spec: https://pixinsight.com/doc/docs/XISF-1.0-spec/XISF-1.0-spec.html
- PixInsight FITS Settings: https://pixinsight.com.ar/en/docs/185/pixinsight-fits-settings.html
- SBIG FITS Extensions (SBFITSEXT): https://www.cloudynights.com/topic/692074-fits-keywords-bayerpat-bayoffx-bayoffy-etc/
- Siril FITS Orientation: https://free-astro.org/index.php?title=Siril:FITS_orientation
- FITS WCS Standard: https://www.aanda.org/articles/aa/full/2002/45/aah3859/aah3859.right.html
- SIP Convention: https://stwcs.readthedocs.io/en/latest/fits_convention_tsr/source/sip.html
