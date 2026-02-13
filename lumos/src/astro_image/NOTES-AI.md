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
- **BitPix**: Enum with UInt8/Int16/UInt16/Int32/UInt32/Int64/Float32/Float64 variants
  - `from_fits_value()` returns `Result<Self, String>` for safe external input handling
  - `normalization_max()` returns divisor for [0,1] normalization (None for floats)
  - `to_fits_value()` maps back to FITS BITPIX integer (unsigned maps to signed equivalent)
- FITS loading via `fitsio` crate (thin wrapper around cfitsio C library)
- All pixel data stored as f32 normalized to [0,1] (both RAW and FITS)

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
- `from_fits_value()` returns Result for safe handling of unknown BITPIX values
- X-Trans demosaic uses `process_xtrans_f32()` — no f32→u16 precision loss
- `ImageDimensions::sample_count()` (w*h*c) vs `pixel_count()` (w*h) — clear naming
- BAYERPAT/ROWORDER/XBAYROFF/YBAYROFF FITS header parsing for CFA pattern detection
- CfaPattern: `from_bayerpat()`, `flip_vertical()`, `flip_horizontal()` for FITS CFA
- Comprehensive FITS metadata: FILTER, GAIN, EGAIN, CCD-TEMP, IMAGETYP, XBINNING,
  YBINNING, SET-TEMP, OFFSET, FOCALLEN, AIRMASS (with CCDTEMP/FRAME fallbacks)

## Remaining Issues

### P7: No FITS Writing Support
**File:** `mod.rs`

`save()` only supports PNG/JPEG/TIFF. FITS is the interchange format for astrophotography.
Also: `save()` clones the entire image which is wasteful.

## FITS Standard Compliance Notes

### BZERO/BSCALE Handling
cfitsio automatically applies BZERO/BSCALE when reading images. For BITPIX=16 +
BZERO=32768 (unsigned 16-bit convention), cfitsio returns values in the 0-65535 range
as f32. The loader then normalizes to [0,1] by dividing by `BitPix::normalization_max()`.

### 3D FITS Data Order
FITS uses Fortran-style (column-major) storage. For NAXIS1=W, NAXIS2=H, NAXIS3=3:
data is stored as W*H red values, then W*H green, then W*H blue. The fitsio-rs crate
reports shape in C order: [3, H, W]. The loader uses `from_planar_channels()` for
3-channel FITS to correctly handle this planar layout.

### Integer Data Convention
FITS standard (4.0) only supports signed integers natively. Unsigned integers use the
BZERO convention: BITPIX=16 + BZERO=32768 + BSCALE=1 for unsigned 16-bit. This is by
far the most common format from astronomical cameras. cfitsio handles this transparently,
and `image_type_to_bitpix()` preserves the unsigned/signed distinction.

## References
- FITS Standard 4.0: https://fits.gsfc.nasa.gov/standard40/fits_standard40aa.pdf
- FITS Dictionary: https://heasarc.gsfc.nasa.gov/docs/fcg/standard_dict.html
- cfitsio Unsigned Int Support: https://heasarc.gsfc.nasa.gov/docs/software/fitsio/c/c_user/node23.html
- cfitsio Data Scaling: https://heasarc.gsfc.nasa.gov/fitsio/c/c_user/node26.html
- MaxIm DL FITS Keywords: https://cdn.diffractionlimited.com/help/maximdl/FITS_File_Header_Definitions.htm
- N.I.N.A. FITS Keywords: https://nighttime-imaging.eu/docs/master/site/advanced/file_formats/fits/
- Siril FITS Format: https://siril.readthedocs.io/en/latest/file-formats/FITS.html
- PixInsight XISF Spec: https://pixinsight.com/doc/docs/XISF-1.0-spec/XISF-1.0-spec.html
