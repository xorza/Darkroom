# astro_image Module - Implementation Review

## Overview
Core data structure (`AstroImage`) for astronomical image processing. Planar f32 pixel
storage, FITS/RAW/standard image I/O, CFA handling, calibration operations.

Files: `mod.rs` (AstroImage, PixelData, ImageDimensions, conversions), `fits.rs` (FITS
loading via cfitsio/fitsio-rs), `sensor.rs` (libraw sensor detection), `cfa.rs` (CfaImage
for raw CFA data, calibration ops, demosaic dispatch).

## Architecture
- **PixelData**: Enum of `L(Buffer2<f32>)` or `Rgb([Buffer2<f32>; 3])` - planar storage
- **AstroImage**: PixelData + ImageDimensions + AstroImageMetadata
- **CfaImage**: Single-channel raw sensor data with CFA pattern metadata
- **ImageDimensions**: width/height/channels (1 or 3 only)
- FITS loading via `fitsio` crate (thin wrapper around cfitsio C library)
- All pixel data stored as f32 (RAW normalized to [0,1], FITS as raw ADU values)

## What It Does Well
- Planar storage matches PixInsight XISF convention for per-channel processing
- Type-safe PixelData enum prevents channel count mismatches
- Correct Rec. 709 luminance coefficients (0.2126, 0.7152, 0.0722)
- Proper sensor detection from libraw filters field (Bayer, X-Trans, Mono)
- Standard flat normalization formula: `light /= (flat - bias) / mean(flat - bias)`
- Rayon parallelism throughout (row-based chunking for cache locality)
- Good test coverage (~50 tests across module)
- Trait-based `StackableImage` abstraction for both AstroImage and CfaImage

## Issues Found

### P0: 3-Channel FITS Loaded as Interleaved Instead of Planar
**File:** `fits.rs:63`

FITS standard stores 3D images in planar order per NAXIS3: all R pixels, then all G, then
all B (confirmed by FITS Standard 4.0, MaxIm DL docs, and cfitsio documentation). The
`fitsio` crate's `read_image()` returns these as a flat Vec in this planar order.

The code calls `AstroImage::from_pixels()` which treats input as interleaved (RGBRGB...).
This produces garbled colors for any 3-channel FITS file. Currently masked because test
files are all grayscale.

**Fix:** Use `from_planar_channels()` for 3-channel FITS:
```rust
if img_dims.channels == 3 {
    let plane_size = img_dims.width * img_dims.height;
    let channels = pixels.chunks_exact(plane_size).map(|c| c.to_vec());
    AstroImage::from_planar_channels(img_dims, channels)
} else {
    AstroImage::from_pixels(img_dims, pixels)
}
```

### P1: Inconsistent Normalization Between FITS and RAW
**File:** `fits.rs:43-45`

RAW files are normalized to [0,1] by libraw (see `raw/NOTES-AI.md`). FITS integer data
retains raw ADU values (e.g., 0-65535 for 16-bit). This creates inconsistency:
- Operations mixing FITS and RAW sources produce nonsensical results
- Threshold-based algorithms break across formats
- Display expects [0,1] range

**Industry practice:**
- PixInsight normalizes all data to [0,1] float internally
- Siril uses [0,1] for 32-bit float processing
- AstroPixelProcessor normalizes to [0,1]

**cfitsio note:** cfitsio automatically applies BZERO/BSCALE when reading via
`fits_read_img` with TFLOAT output. So a BITPIX=16, BZERO=32768 file (unsigned 16-bit)
is already converted to unsigned values (0-65535 range). The remaining step is dividing
by the max value for the BITPIX type to get [0,1].

**Fix:** After `read_image`, normalize based on original BITPIX:
- UInt8: divide by 255.0
- Int16/UInt16: divide by 65535.0 (cfitsio already applied BZERO)
- Int32/UInt32: divide by max value
- Float32/Float64: assume already normalized (or check range)

### P2: BitPix Enum Conflates Unsigned and Signed Types
**File:** `fits.rs:69-78`, `mod.rs:20-53`

`image_type_to_bitpix()` maps both `UnsignedShort` and `Short` to `BitPix::Int16`. UInt16
is the dominant format in amateur astrophotography (virtually all cooled CCD/CMOS cameras).
This loses the distinction needed for:
- Correct normalization range (32767 vs 65535)
- Proper FITS writing with BZERO convention
- Metadata fidelity

**Note:** cfitsio reports `USHORT_IMG` when BITPIX=16 + BZERO=32768, so the fitsio crate
does distinguish at the `ImageType` level - this info is just discarded during mapping.

**Fix:** Add `UInt8`, `UInt16`, `UInt32` variants to BitPix enum.

### P3: Missing BAYERPAT FITS Header Reading
**File:** `fits.rs:50-51`

Comment says "FITS files don't indicate CFA status" - this is incorrect. BAYERPAT is a
widely used non-standard keyword written by:
- N.I.N.A. (BAYERPAT, XBAYROFF, YBAYROFF)
- SharpCap (BAYERPAT)
- SGPro (BAYERPAT)
- MaxIm DL (BAYERPAT)
- Most modern capture software

Values: "RGGB", "BGGR", "GRBG", "GBRG".

Additionally, ROWORDER keyword ("TOP-DOWN" or "BOTTOM-UP") affects CFA pattern
interpretation. Siril reads ROWORDER and flips the pattern accordingly.

**Fix:**
```rust
let cfa_type: Option<CfaType> = read_key_optional::<String>(&hdu, &mut fptr, "BAYERPAT")
    .and_then(|val| match val.trim() {
        "RGGB" => Some(CfaType::Bayer(CfaPattern::Rggb)),
        "BGGR" => Some(CfaType::Bayer(CfaPattern::Bggr)),
        "GRBG" => Some(CfaType::Bayer(CfaPattern::Grbg)),
        "GBRG" => Some(CfaType::Bayer(CfaPattern::Gbrg)),
        _ => None,
    });
```

### P4: Missing Standard FITS Metadata Fields
**File:** `mod.rs:102-114`

Currently reads: OBJECT, INSTRUME, TELESCOP, DATE-OBS, EXPTIME.
Missing fields used by N.I.N.A., Siril, PixInsight, MaxIm DL:

| Keyword | Purpose | Used by |
|---------|---------|---------|
| FILTER | Filter name (Ha, OIII, L, R, G, B) | All tools |
| GAIN | Camera gain setting | N.I.N.A., Siril |
| EGAIN | Electrons per ADU | MaxIm DL, N.I.N.A. |
| CCD-TEMP | Sensor temperature (C) | All tools |
| IMAGETYP | Frame type (LIGHT, DARK, FLAT, BIAS) | All tools |
| XBINNING/YBINNING | Pixel binning factors | All tools |
| BAYERPAT | CFA pattern | See P3 |
| SET-TEMP | Target cooling temperature | N.I.N.A. |
| OFFSET | Camera offset/bias setting | N.I.N.A. |
| FOCALLEN | Focal length (mm) | MaxIm DL, N.I.N.A. |
| AIRMASS | Atmospheric extinction factor | MaxIm DL |

Impact: Incomplete metadata propagation through calibration/stacking pipeline. FILTER
is critical for multi-narrowband workflows. IMAGETYP enables automatic frame classification.

### P5: pixel_count() Returns Sample Count, Not Pixel Count
**File:** `mod.rs:83-85`

Returns `width * height * channels`. For 100x100 RGB: returns 30000, not 10000.
Name implies spatial pixel count. Used in `from_pixels()` assertion and FITS loader.

**Fix:** Rename to `sample_count()` and add true `pixel_count() -> width * height`.

### P6: X-Trans Demosaic Roundtrips Through u16
**File:** `cfa.rs:121-137`

Converts f32 -> u16 -> f32, losing ~15 bits of precision (1/65536 quantization).
Bayer path works directly on f32. This is a workaround for the X-Trans pipeline
accepting u16 input.

**Fix:** Modify X-Trans demosaic to accept `&[f32]` directly, matching Bayer path.

### P7: No FITS Writing Support
**File:** `mod.rs:467-471`

`save()` only supports PNG/JPEG/TIFF. FITS is the interchange format for astrophotography.
All major tools (Siril, PixInsight, APP) use FITS for intermediate and final output.
Also: `save()` clones the entire image (`.clone().into()`) which is wasteful.

### P8: from_fits_value() Panics on Unknown BITPIX
**File:** `mod.rs:31-41`

BITPIX comes from external user files. Per project error handling rules, external input
should use `Result<>`, not panic. An unknown BITPIX value is an expected failure mode
(corrupted or non-standard FITS files).

## FITS Standard Compliance Notes

### BZERO/BSCALE Handling
cfitsio automatically applies BZERO/BSCALE when reading images, so the fitsio Rust crate
inherits this behavior. For BITPIX=16 + BZERO=32768 (unsigned 16-bit convention), cfitsio
returns values in the 0-65535 range as f32. No manual BZERO/BSCALE handling is needed in
the Rust code - only post-read normalization to [0,1].

### 3D FITS Data Order
FITS uses Fortran-style (column-major) storage. For NAXIS1=W, NAXIS2=H, NAXIS3=3:
data is stored as W*H red values, then W*H green, then W*H blue. The fitsio-rs crate
reports shape in C order: [3, H, W]. The `read_image()` returns a flat Vec in this
planar order. Current code correctly parses dimensions but incorrectly treats the flat
pixel data as interleaved.

### Integer Data Convention
FITS standard (4.0) only supports signed integers natively. Unsigned integers use the
BZERO convention: BITPIX=16 + BZERO=32768 + BSCALE=1 for unsigned 16-bit. This is by
far the most common format from astronomical cameras. cfitsio handles this transparently.

## Recommendations by Priority

1. **Immediate (P0-P1):** Fix planar FITS loading (data corruption for color FITS).
   Add normalization for integer FITS data (format consistency).

2. **Short-term (P2-P4):** Add UInt16/UInt32 BitPix variants. Read BAYERPAT + ROWORDER
   from FITS headers. Add FILTER, GAIN, CCD-TEMP, IMAGETYP to metadata.

3. **Medium-term (P5-P8):** Rename pixel_count, fix X-Trans precision loss, add FITS
   writing, convert from_fits_value panic to Result.

## References
- FITS Standard 4.0: https://fits.gsfc.nasa.gov/standard40/fits_standard40aa.pdf
- FITS Dictionary: https://heasarc.gsfc.nasa.gov/docs/fcg/standard_dict.html
- cfitsio Unsigned Int Support: https://heasarc.gsfc.nasa.gov/docs/software/fitsio/c/c_user/node23.html
- cfitsio Data Scaling: https://heasarc.gsfc.nasa.gov/fitsio/c/c_user/node26.html
- MaxIm DL FITS Keywords: https://cdn.diffractionlimited.com/help/maximdl/FITS_File_Header_Definitions.htm
- N.I.N.A. FITS Keywords: https://nighttime-imaging.eu/docs/master/site/advanced/file_formats/fits/
- Siril FITS Format: https://siril.readthedocs.io/en/latest/file-formats/FITS.html
- PixInsight XISF Spec: https://pixinsight.com/doc/docs/XISF-1.0-spec/XISF-1.0-spec.html
