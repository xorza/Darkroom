# astro_image Module - Code Review vs Industry Standards

## Overview
Core data structure (`AstroImage`) for astronomical image processing. Planar f32 pixel storage, FITS/RAW/image file I/O, CFA handling, calibration operations.

## What It Does Well
- Planar storage design matches PixInsight XISF standard
- Type-safe pixel data enum (L vs Rgb)
- Correct Rec. 709 luminance coefficients (0.2126, 0.7152, 0.0722)
- Proper sensor detection from libraw filters field
- Standard calibration formula for flat division
- Parallel processing with rayon throughout
- Good test coverage (30+ tests)

## Issues Found

| Priority | Issue | Impact |
|----------|-------|--------|
| P0 | 3-Channel FITS Loaded as Interleaved Instead of Planar | Data corruption for color FITS files |
| P1 | No Normalization of FITS Integer Data | Inconsistent value ranges, broken operations |
| P2 | BitPix Enum Conflates Unsigned and Signed Types | Incorrect FITS writing, lost metadata |
| P3 | Missing BAYERPAT FITS Header Reading | CFA pattern detection fails for modern FITS |
| P4 | Missing Standard FITS Metadata Fields | Incompatibility with SIRIL/PixInsight workflows |
| P5 | pixel_count() Name is Misleading | API confusion (returns samples, not pixels) |
| P6 | X-Trans Demosaic Roundtrips Through u16 | Precision loss from quantization |
| P7 | No FITS Writing Support | Cannot export to standard astro format |
| P8 | from_fits_value() Panics on Unknown BITPIX | Violates error handling rules |

### P0: 3-Channel FITS Loaded as Interleaved Instead of Planar
**File:** `fits.rs:41-46`

**Problem:**
- FITS stores 3D images in planar order: all R, all G, all B (per FITS standard)
- `AstroImage::from_pixels()` treats input as interleaved (RGBRGB...)
- Produces garbled color for 3-channel FITS files
- Currently masked because test files are all grayscale

**Fix:**
```rust
if channels == 3 {
    AstroImage::from_planar_channels(r_plane, g_plane, b_plane, width, height)
} else {
    AstroImage::from_pixels(...)
}
```

### P1: No Normalization of FITS Integer Data
**File:** `fits.rs:41-46`

**Problem:**
- RAW files normalized to [0, 1] by libraw
- FITS integers kept as raw ADU values (0-65535 for 16-bit)
- Creates major inconsistency between formats
- Operations between FITS and RAW images produce nonsensical results

**Fix:**
Normalize integer BITPIX types to [0, 1] on load:
- Int16: divide by 32767.0
- UInt16: divide by 65535.0
- Int32: divide by 2147483647.0
- UInt32: divide by 4294967295.0

### P2: BitPix Enum Conflates Unsigned and Signed Types
**File:** `fits.rs:71-76`, `mod.rs:30-50`

**Problem:**
- `UnsignedShort` maps to `BitPix::Int16` - no UInt16/UInt32 variants
- UInt16 is the most common format in amateur astrophotography
- Loses distinction needed for FITS writing (BZERO convention)
- Signed vs unsigned requires different normalization ranges

**Fix:**
Add enum variants:
```rust
pub enum BitPix {
    Float32,
    Int16,
    UInt16,  // new
    Int32,
    UInt32,  // new
}
```

### P3: Missing BAYERPAT FITS Header Reading
**File:** `fits.rs:48-56`

**Problem:**
- Code comments say "FITS files don't indicate CFA status" - incorrect
- SharpCap, N.I.N.A., SGPro all write `BAYERPAT` keyword
- Common values: "RGGB", "BGGR", "GRBG", "GBRG"
- Missing this means CFA pattern detection fails for modern FITS

**Fix:**
```rust
let cfa_pattern = header.get("BAYERPAT")
    .and_then(|val| match val.as_str() {
        "RGGB" => Some(CfaPattern::Rggb),
        "BGGR" => Some(CfaPattern::Bggr),
        "GRBG" => Some(CfaPattern::Grbg),
        "GBRG" => Some(CfaPattern::Gbrg),
        _ => None,
    });
```

### P4: Missing Standard FITS Metadata Fields
**File:** `mod.rs:103-116`

**Problem:**
Missing metadata fields commonly used by SIRIL and PixInsight:
- `FILTER` - filter name (Ha, OIII, L, R, G, B)
- `GAIN`/`EGAIN` - sensor gain in electrons per ADU
- `CCD-TEMP` - sensor temperature in Celsius
- `AIRMASS` - atmospheric extinction correction
- `XBINNING`/`YBINNING` - pixel binning factors
- `FOCALLEN` - focal length in mm
- `BSCALE`/`BZERO` - linear transformation for integer data

**Impact:**
- Incomplete metadata propagation through calibration pipeline
- Incompatibility with standard preprocessing workflows

### P5: pixel_count() Name is Misleading
**File:** `mod.rs:83-85`

**Problem:**
- Returns `width * height * channels` (total samples)
- For 100x100 RGB image: returns 30000, not 10000
- Name implies pixel count, not sample count

**Fix:**
```rust
pub fn sample_count(&self) -> usize {
    self.width * self.height * self.channel_count()
}

pub fn pixel_count(&self) -> usize {
    self.width * self.height
}
```

### P6: X-Trans Demosaic Roundtrips Through u16
**File:** `cfa.rs:103-119`

**Problem:**
- f32 → u16 → f32 conversion loses precision
- Quantization error: 1/65536 ≈ 0.0000153
- Bayer path works directly on f32
- Unnecessary precision loss in X-Trans processing

**Fix:**
Either:
1. Modify `demosaic_xtrans` to accept `&[f32]`
2. Or use `u32` for conversion (1/4294967296 precision)

### P7: No FITS Writing Support
**File:** `mod.rs:340-344`

**Problem:**
- `save()` only supports PNG/JPEG/TIFF via imaginarium
- FITS is the interchange format for astrophotography
- All major tools (SIRIL, PixInsight, APP) use FITS for intermediate files
- Also: `save()` clones the entire image unnecessarily

**Fix:**
Add `save_fits()` method or extend `save()` to detect `.fits` extension

### P8: from_fits_value() Panics on Unknown BITPIX
**File:** `mod.rs:44-46`

**Problem:**
- BITPIX comes from external files (user input)
- Panics on unknown values instead of returning `Result`
- Violates project error handling rules: "Use `Result<>` for expected failures (network, I/O, external services, user input)"

**Fix:**
```rust
pub fn from_fits_value(value: i32) -> Result<Self, String> {
    match value {
        -32 => Ok(BitPix::Float32),
        16 => Ok(BitPix::Int16),
        32 => Ok(BitPix::Int32),
        _ => Err(format!("Unsupported BITPIX value: {}", value)),
    }
}
```

## Standards References
- FITS Standard 4.0: https://fits.gsfc.nasa.gov/standard40/fits_standard40aa-le.pdf
- PixInsight XISF: https://pixinsight.com/xisf/
- BAYERPAT convention: https://www.astro.louisville.edu/software/sbig/archive/sbwhtmls/bayerpat.htm
- Rec. 709 (used correctly): ITU-R BT.709

## Recommendations by Priority

1. **Immediate (P0-P1):**
   - Fix planar FITS loading (data corruption)
   - Normalize FITS integer data (consistency)

2. **Short-term (P2-P4):**
   - Add UInt16/UInt32 to BitPix enum
   - Read BAYERPAT from FITS headers
   - Add standard FITS metadata fields

3. **Medium-term (P5-P8):**
   - Rename pixel_count() → sample_count()
   - Fix X-Trans precision loss
   - Add FITS writing support
   - Convert from_fits_value() panic to Result
