# astro_image Module

## Module Overview

Core data structure (`AstroImage`) for astronomical image processing. Planar f32 pixel
storage, FITS/RAW/standard image I/O, CFA handling, calibration operations.

**Files:**
- `mod.rs` -- AstroImage, PixelData, ImageDimensions, BitPix, conversions, arithmetic ops
- `fits.rs` -- FITS loading via cfitsio/fitsio-rs, normalization, CFA header parsing
- `sensor.rs` -- libraw sensor type detection from filters/colors fields
- `cfa.rs` -- CfaImage for raw CFA data, calibration ops (dark/flat), demosaic dispatch

**Architecture:**
- `PixelData`: Enum `L(Buffer2<f32>)` | `Rgb([Buffer2<f32>; 3])` -- planar storage
- `AstroImage`: PixelData + ImageDimensions + AstroImageMetadata
- `CfaImage`: Single-channel raw sensor data with CFA pattern metadata
- `ImageDimensions`: width/height/channels (only 1 or 3)
- `BitPix`: UInt8/Int16/UInt16/Int32/UInt32/Int64/Float32/Float64
- All pixel data stored as f32 normalized to [0,1]

**FITS metadata currently read:**
OBJECT, INSTRUME, TELESCOP, DATE-OBS, EXPTIME, FILTER, GAIN, EGAIN,
CCD-TEMP (fallback CCDTEMP), IMAGETYP (fallback FRAME), XBINNING, YBINNING,
SET-TEMP, OFFSET, FOCALLEN, AIRMASS, BAYERPAT, ROWORDER, XBAYROFF, YBAYROFF.

---

## Industry Standard Comparison

### FITS Standard 4.0 Compliance

**BZERO/BSCALE -- Correct.** cfitsio automatically applies BZERO/BSCALE when reading.
The `image_type_to_bitpix()` correctly maps cfitsio's `ImageType` enum (which includes
`UnsignedShort`, `UnsignedLong`, etc.) to `BitPix` variants, preserving the unsigned/signed
distinction for correct normalization. The FITS standard discourages using BSCALE/BZERO
with floating-point data (BITPIX < 0) due to overflow risk; cfitsio would still apply them
if present, but this is extremely rare in practice.

**3D FITS Data Order -- Correct.** FITS stores 3D image data in planar order (NAXIS3 planes
of NAXIS1 x NAXIS2 pixels). fitsio-rs reports shape in C order: [channels, height, width].
The loader correctly uses `from_planar_channels()` for 3-channel data.

**Integer Unsigned Convention -- Correct.** FITS only supports signed integers natively.
BITPIX=16 + BZERO=32768 + BSCALE=1 is the standard for unsigned 16-bit (by far the most
common astronomical camera output). cfitsio handles this transparently.

**HDU Structure -- Partial.** Only the primary HDU is read. The FITS standard supports
Image Extensions, ASCII Table Extensions, and Binary Table Extensions. Professional tools
(astropy, Siril, PixInsight) read all HDU types.

### Normalization Comparison

| Tool | Integer types | Float types |
|------|--------------|-------------|
| **Astropy** | BZERO+BSCALE to float (no [0,1]) | Raw values preserved |
| **PixInsight** | BZERO/BSCALE then /max to [0,1] | Assumes [0,1] default (configurable) |
| **Siril** | BZERO/BSCALE to USHORT or FLOAT[0,1] | If max > 1, divide by 65535 |
| **DeepSkyStacker** | Standard | Writes float in [0,65535]; reads DATAMIN/DATAMAX |
| **Our impl** | /type_max to [0,1] | Heuristic: if max > 2.0, divide by max |

Our float heuristic (divide by max when max > 2.0) is a reasonable middle ground. It handles
DeepSkyStacker [0,65535] output and arbitrary-range scientific data. The 2.0 threshold provides
headroom for HDR/overexposed pixels while catching integer-like ranges. This is slightly
better than Siril's fixed /65535 approach because it adapts to any range.

**Gap vs Siril:** Siril always divides by 65535 for non-[0,1] float data, which breaks if
the data is in a different range (e.g., [0,255] or [0,100000]). Our max-based approach
handles all ranges correctly.

**Gap vs Astropy:** Astropy does not normalize at all -- it preserves physical units. This
is correct for scientific analysis but wrong for astrophotography processing pipelines
that assume [0,1]. Our approach is correct for our use case.

**Gap vs PixInsight:** PixInsight's configurable range is more flexible but adds user
complexity. Our auto-detection is simpler and handles the common cases well.

### CFA/Bayer Handling

**BAYERPAT Convention -- Correct and thorough.**
BAYERPAT/XBAYROFF/YBAYROFF originated from the SBIG SBFITSEXT specification (2003). Not
part of the official FITS standard but universally used by amateur astronomy cameras and
capture software (NINA, MaximDL, CCDSoft, SGP).

Our handling:
- Supports RGGB, BGGR, GRBG, GBRG, TRUE (=RGGB)
- Adjusts for ROWORDER (BOTTOM-UP flips pattern vertically)
- Handles XBAYROFF/YBAYROFF integer offsets
- Matches NINA, MaximDL, Siril, PixInsight behavior

**CFA Calibration -- Correct.**
Per-CFA-channel flat normalization prevents color shift from non-white flat light sources.
This matches how PixInsight and Siril handle flat correction for Bayer/X-Trans data.
The formula `light /= (flat - bias) / mean_color(flat - bias)` is industry-standard.

**Sensor Detection -- Correct.**
`detect_sensor_type()` correctly interprets libraw's `filters` and `colors` fields:
filters=0 or colors=1 for Mono, filters=9 for X-Trans, standard bit patterns for Bayer.

### Comparison with Specific Tools

**vs cfitsio:**
- cfitsio provides C-level FITS I/O with full standard compliance
- We use it correctly through fitsio-rs wrapper
- cfitsio handles compressed FITS transparently (Rice, GZIP, HCompress, PLIO)
- We benefit from this for reading but cannot write compressed FITS

**vs Astropy (Python):**
- Astropy preserves physical units, does not normalize to [0,1]
- Astropy supports full WCS, tables, multi-HDU, CHECKSUM/DATASUM
- Astropy handles BSCALE/BZERO with automatic header updates
- Astropy is a general-purpose tool; our loader is specialized for astrophotography

**vs Siril:**
- Siril uses USHORT (16-bit) or FLOAT [0,1] internally
- Siril converts 8-bit to 16-bit, 32-bit int to float, 64-bit float to 32-bit
- Siril divides non-[0,1] float by 65535 (less flexible than our max-based heuristic)
- Siril reads ROWORDER for Bayer pattern only (matches our behavior)
- Siril reads more metadata (RA/DEC, XPIXSZ/YPIXSZ, CALSTAT)

**vs PixInsight:**
- PixInsight uses planar [0,1] float internally (matches our architecture)
- PixInsight reads/writes FITS with configurable float range
- PixInsight supports XISF (its native format) with richer metadata
- PixInsight reads full WCS, all metadata keywords

---

## Missing Features (with Severity)

### Critical -- None

The module correctly handles the core use case: loading FITS images from astronomical
cameras, normalizing to [0,1], and providing calibration operations.

### High

**H1: No FITS Writing Support.**
`save()` only supports PNG/JPEG/TIFF. FITS is the primary interchange format for
astrophotography. Stacking results, calibrated masters, and processed images should
be saveable as FITS with metadata preserved. Every other tool in this space (Siril,
PixInsight, DSS, ASTAP) writes FITS output.

**H2: Missing RA/DEC Metadata.**
RA and DEC (telescope pointing coordinates) are written by every capture application
(NINA, MaximDL, SGP, APT). They are essential for: plate solving initialization,
mosaic planning, target identification, and sky atlas overlays.

**H3: Missing Pixel Size (XPIXSZ/YPIXSZ).**
Needed for plate scale calculation: `plate_scale = pixel_size / focal_length * 206.265`.
Written by NINA, MaximDL, and most camera drivers. Required for automated field
identification and astrometry.

### Medium

**M1: Missing DATAMAX/DATAMIN Keywords.**
DATAMAX indicates the saturation threshold. Essential for star detection (distinguishing
saturated stars), ADU clipping detection, and data range validation. DeepSkyStacker uses
these keywords for float FITS range detection.

**Apple laptop: Missing FOCRATIO (Focal Ratio).**
Written by NINA. Useful for optical calculations and metadata display.

**M3: Missing OBJCTRA/OBJCTDEC (Target Coordinates).**
Target coordinates in sexagesimal format (HH MM SS / DD MM SS). Distinct from RA/DEC
(which are telescope pointing). Used by plate solvers and catalog cross-referencing.

**M4: Missing SITELAT/SITELONG/SITEELEV (Observatory Location).**
Written by NINA. Needed for atmospheric refraction correction and precise timing.

**Apple laptop: No Multi-HDU Support.**
Only the primary HDU is read. Compressed FITS stores the image in an extension HDU.
Some observatories use multi-extension FITS. cfitsio can read named/numbered HDUs but
the loader does not attempt this.

**M6: Missing CALSTAT (Calibration Status).**
Tracks which calibration frames have been applied (B=bias, D=dark, F=flat). Prevents
accidental double-calibration.

**M7: ISO Not Read from FITS.**
The `iso` field is always `None` for FITS files. NINA writes ISOSPEED for DSLR cameras.

### Low

**L1: Missing READOUTM (Readout Mode).**
Camera readout mode (e.g., "Normal", "Low Noise"). Written by NINA.

**L2: Missing DARKTIME.**
Dark current integration time. May differ from EXPTIME for cameras with mechanical
shutters. Used for dark frame scaling.

**L3: Missing PEDESTAL.**
ADU offset added during calibration. Some cameras/software add a pedestal to prevent
negative values.

**L4: No FITS CHECKSUM/DATASUM Support.**
Data integrity verification defined in the FITS Checksum Convention. Detects file
corruption. Supported by cfitsio, astropy, and professional observatories.

**L5: Missing Weather/Environment Keywords.**
NINA writes AMBTEMP, HUMIDITY, PRESSURE, DEWPOINT, MPSAS, SKYTEMP, WINDSPD, etc.
Useful for data quality filtering but not critical for image processing.

**L6: Missing SWCREATE/OBSERVER.**
Software creator and observer name. Metadata only; no processing impact.

---

## Correctness Issues

### Issue 1: Float Normalization Heuristic Could Misfire on Unusual Data

**Severity: Low (heuristic is reasonable for target use case)**

The heuristic normalizes float FITS data by dividing by max when max > 2.0. Edge cases:

1. **Physical unit FITS** (e.g., flux in Jy) with max = 1.5: correctly left alone, but
   downstream processing assumes [0,1] and would produce wrong results.
2. **All-negative data** (e.g., fully dark-subtracted): max could be < 2.0 with values
   like [-0.5, 1.8], left unnormalized. This is actually correct behavior.
3. **Single bright pixel at 3.0** with rest in [0,1]: would divide everything by 3.0,
   potentially squashing valid HDR data.

**Improvement:** Check DATAMAX/DATAMIN keywords first (if present, use those for range
detection). Fall back to max-based heuristic only when keywords are absent.

### Issue 2: Int16 Normalization Asymmetry (Minor)

**Severity: Negligible**

Int16 normalization divides by 32767. This maps -32768 to -1.000031 (slightly beyond -1.0).
The asymmetry is 1 part in 32768, which is negligible for astronomical processing. The Int16
path is also very rare -- nearly all astronomical cameras use the BZERO=32768 unsigned
convention, which maps to UInt16.

### Issue 3: NaN/Inf Handling in FITS Data — FIXED

`normalize_fits_pixels()` sanitizes NaN/Inf to 0.0 for float BitPix types before
normalization. Integer types are not sanitized (cfitsio should never produce NaN for
integer data). This ensures the invariant that source images are NaN-free after loading.

### Issue 4: ROWORDER Handling is Correct

**Not a bug.** ROWORDER is used only for Bayer pattern adjustment, not for pixel flipping.
This matches Siril's behavior and is the pragmatic choice. Most capture software writes
TOP-DOWN despite the FITS standard recommending first pixel at lower-left.

---

## Unnecessary Complexity

### None Found

The module is well-structured with clear separation of concerns:

- `PixelData` enum is the right abstraction for planar L/RGB data
- `BitPix` enum correctly models the FITS type system including unsigned variants
- `CfaType` enum properly handles Mono/Bayer/X-Trans
- The flat normalization has the right mono/CFA branching
- Test coverage is thorough with hand-computed expected values

The `From<Image>` implementation has some code duplication between packed and strided paths,
but this is justified by the performance difference (avoiding unnecessary per-pixel stride
calculations for packed data).

The `to_image()` and `Into<Image>` implementations are functionally identical. `to_image()`
borrows while `Into<Image>` consumes. Having both is reasonable for different use cases.

---

## Recommendations

### Priority 1: Add FITS Writing

Implement `save_fits()` using cfitsio's write functions. Should preserve all metadata
from `AstroImageMetadata`. Write as BITPIX=-32 (float32) normalized [0,1] to match
PixInsight convention. Consider optional BITPIX=16 with BZERO=32768 for compatibility
with tools that prefer integer FITS.

### Priority 2: Add RA/DEC and Pixel Size Metadata

Add fields to `AstroImageMetadata`:
- `ra: Option<f64>` (degrees), `dec: Option<f64>` (degrees)
- `pixel_size_x: Option<f64>` (microns), `pixel_size_y: Option<f64>` (microns)
- `focal_ratio: Option<f64>`

Read from FITS: RA, DEC, XPIXSZ, YPIXSZ, FOCRATIO.

### Priority 3: Read DATAMAX/DATAMIN for Float Normalization

When loading float FITS, check DATAMAX keyword first. If present and > 2.0, use it as
the normalization divisor instead of computing max from pixel data. This is more
reliable than scanning all pixels and handles the standard correctly.

### Priority 4: Read ISOSPEED from FITS

Trivial fix: add `read_key_optional(&hdu, &mut fptr, "ISOSPEED")` for the `iso` field
in the FITS metadata reader.

### ~~Priority 5: NaN Handling~~ — DONE

Float FITS NaN/Inf sanitized to 0.0 in `normalize_fits_pixels()`. 6 tests added.

### Priority 6: Multi-HDU Support (When Needed)

When encountering an empty primary HDU, try reading the first image extension. This
handles compressed FITS and some observatory formats.

---

## Memory Layout

**Planar storage** (`PixelData::Rgb([Buffer2<f32>; 3])`):
- Each channel is a contiguous `Vec<f32>` of width*height elements
- Row-major order within each channel
- Matches PixInsight XISF internal format
- Optimal for per-channel operations (statistics, convolution, stacking)
- Suboptimal for per-pixel RGB operations (cache misses across 3 buffers)

**Trade-off:** Planar is the right choice for astro processing. Per-channel operations
dominate the processing pipeline.

---

## References

- FITS Standard 4.0: https://fits.gsfc.nasa.gov/standard40/fits_standard40aa.pdf
- FITS Dictionary: https://heasarc.gsfc.nasa.gov/docs/fcg/standard_dict.html
- FITS Primer: https://fits.gsfc.nasa.gov/fits_primer.html
- FITS User's Guide: https://fits.gsfc.nasa.gov/users_guide/usersguide.pdf
- cfitsio Data Scaling: https://heasarc.gsfc.nasa.gov/fitsio/c/c_user/node26.html
- cfitsio Image Compression: https://heasarc.gsfc.nasa.gov/docs/software/fitsio/c/c_user/node41.html
- cfitsio Unsigned Int Support: https://heasarc.gsfc.nasa.gov/docs/software/fitsio/c/c_user/node23.html
- Astropy FITS Image Data: https://docs.astropy.org/en/stable/io/fits/usage/image.html
- Astropy FITS Verification: https://docs.astropy.org/en/stable/io/fits/usage/verification.html
- Siril FITS Format: https://siril.readthedocs.io/en/latest/file-formats/FITS.html
- Siril Supported FITS: https://free-astro.org/index.php?title=Siril:supported_FITS
- Siril FITS Format Handling (DeepWiki): https://deepwiki.com/gnthibault/siril/4.1-fits-format-handling
- Siril FITS Orientation: https://free-astro.org/index.php?title=Siril:FITS_orientation
- PixInsight XISF Spec: https://pixinsight.com/doc/docs/XISF-1.0-spec/XISF-1.0-spec.html
- PixInsight FITS Settings: https://pixinsight.com.ar/en/docs/185/pixinsight-fits-settings.html
- PixInsight BZERO/BSCALE Forum: https://pixinsight.com/forum/index.php?threads/bzero-and-bscale-not-included-in-32bit-float-files.3350/
- MaxIm DL FITS Keywords: https://cdn.diffractionlimited.com/help/maximdl/FITS_File_Header_Definitions.htm
- N.I.N.A. FITS Keywords: https://nighttime-imaging.eu/docs/master/site/advanced/file_formats/fits/
- SBIG FITS Extensions (SBFITSEXT): https://www.cloudynights.com/topic/692074-fits-keywords-bayerpat-bayoffx-bayoffy-etc/
- FITS WCS Standard (Greisen & Calabretta 2002): https://www.aanda.org/articles/aa/full/2002/45/aah3859/aah3859.right.html
- SIP Convention: https://stwcs.readthedocs.io/en/latest/fits_convention_tsr/source/sip.html
- FITS Checksum Convention: https://heasarc.gsfc.nasa.gov/docs/software/fitsio/compression.html
- fpack/funpack Compression: https://heasarc.gsfc.nasa.gov/fitsio/fpack/
