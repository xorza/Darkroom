# Implementation Plan: Raw CFA Hot Pixel Detection and Correction

## Goal

Move hot pixel detection and correction to operate on raw (un-demosaiced) CFA data, before any interpolation happens. This is the mathematically correct approach used by PixInsight and Siril.

## New Pipeline Order

```
RAW file
  ↓
load_raw_cfa()        ← NEW: returns 1-channel CFA data + pattern info
  ↓
stack raw darks       ← 1-channel stacking (already works, just needs 1ch input)
  ↓
master_dark_raw       ← 1-channel CFA
  ↓
detect hot pixels     ← on raw CFA data, simple threshold (no CFA neighbor logic needed here)
  ↓
HotPixelMap           ← pixel positions (x,y), not per-channel RGB indices
  ↓
For each light frame:
  load_raw_cfa()      → 1-channel CFA data
  hot_pixel_correct() → CFA-aware same-color median replacement on raw data  ← NEW
  dark_subtract()     → pixel-wise subtraction on raw data (trivial, same CFA alignment)
  flat_divide()       → pixel-wise division on raw data (trivial, same CFA alignment)
  demosaic()          → existing demosaic_bayer / process_xtrans → 3-channel RGB
  ↓
AstroImage (3ch RGB, clean)
```

## Data Structures

### 1. `CfaImage` - New struct for raw CFA data

```rust
// Location: lumos/src/astro_image/mod.rs (or new file lumos/src/astro_image/cfa.rs)

/// Raw CFA image - single channel with color filter pattern metadata.
/// Represents sensor data before demosaicing.
#[derive(Debug, Clone)]
pub struct CfaImage {
    /// Single-channel pixel data, normalized to 0.0-1.0.
    /// Layout: row-major, width * height pixels.
    pub pixels: Vec<f32>,
    pub width: usize,
    pub height: usize,
    pub pattern: CfaType,
    pub metadata: AstroImageMetadata,
}
```

### 2. `CfaType` - Unified CFA pattern enum

```rust
// Location: lumos/src/astro_image/cfa.rs

/// CFA pattern for raw sensor data.
#[derive(Debug, Clone)]
pub enum CfaType {
    /// 2x2 Bayer pattern
    Bayer(CfaPattern),
    /// 6x6 X-Trans pattern
    XTrans([[u8; 6]; 6]),
}
```

### 3. `HotPixelMap` - Simplified to position-only

```rust
// Location: lumos/src/stacking/hot_pixels.rs (modify existing)

/// Hot pixel positions detected from a master dark frame.
/// Stores flat indices into a width*height grid (not per-channel).
#[derive(Debug, Clone)]
pub struct HotPixelMap {
    /// Sorted list of flat pixel indices (y * width + x) that are hot.
    pub indices: Vec<usize>,
    pub width: usize,
    pub height: usize,
}
```

The current `HotPixelMask` enum (`L`/`Rgb` variants) becomes unnecessary. On raw CFA data there is only one channel, so hot pixel positions are simply flat indices. The `count` field becomes `indices.len()`.

### 4. `CalibrationMasters` - Add raw dark storage

```rust
// Location: lumos/src/calibration_masters/mod.rs (modify existing)

pub struct CalibrationMasters {
    /// Master dark as raw CFA (1 channel). Used for hot pixel detection
    /// and raw-domain dark subtraction.
    pub master_dark_raw: Option<CfaImage>,
    /// Master flat as raw CFA (1 channel). Used for raw-domain flat division.
    pub master_flat_raw: Option<CfaImage>,
    /// Master bias as raw CFA (1 channel).
    pub master_bias_raw: Option<CfaImage>,
    /// Hot pixel map derived from raw master dark.
    pub hot_pixel_map: Option<HotPixelMap>,
    pub config: StackConfig,
}
```

The old `master_dark: Option<AstroImage>` (3-channel RGB) fields are removed. All calibration masters are stored as raw CFA data.

## Implementation Steps

### Step 1: Create `CfaImage` struct and `CfaType` enum

**File:** `lumos/src/astro_image/cfa.rs` (new file)

```rust
use super::AstroImageMetadata;
use crate::raw::demosaic::CfaPattern;

#[derive(Debug, Clone)]
pub enum CfaType {
    Bayer(CfaPattern),
    XTrans([[u8; 6]; 6]),
}

impl CfaType {
    /// Get the color index (0=R, 1=G, 2=B) at position (x, y).
    pub fn color_at(&self, x: usize, y: usize) -> u8 {
        match self {
            CfaType::Bayer(p) => p.color_at(y, x) as u8,
            CfaType::XTrans(pattern) => pattern[y % 6][x % 6],
        }
    }

    /// Get the CFA period (2 for Bayer, 6 for X-Trans).
    pub fn period(&self) -> usize {
        match self {
            CfaType::Bayer(_) => 2,
            CfaType::XTrans(_) => 6,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CfaImage {
    pub pixels: Vec<f32>,
    pub width: usize,
    pub height: usize,
    pub pattern: CfaType,
    pub metadata: AstroImageMetadata,
}

impl CfaImage {
    pub fn pixel_count(&self) -> usize {
        self.width * self.height
    }

    /// Demosaic this CFA image into a 3-channel AstroImage.
    /// Consumes self.
    pub fn demosaic(self) -> AstroImage { ... }

    /// Subtract another CfaImage pixel-by-pixel (dark subtraction).
    pub fn subtract(&mut self, other: &CfaImage) { ... }

    /// Divide by another CfaImage pixel-by-pixel (flat correction).
    pub fn divide_by_normalized(&mut self, flat: &CfaImage, bias: Option<&CfaImage>) { ... }
}
```

**Exports:** Add `pub mod cfa;` to `lumos/src/astro_image/mod.rs`. Export `CfaImage` and `CfaType` from `lib.rs`.

### Step 2: Add `load_raw_cfa()` function

**File:** `lumos/src/raw/mod.rs`

Add a new public function alongside `load_raw()`:

```rust
/// Load raw file and return un-demosaiced CFA data.
/// Returns single-channel f32 data with CFA pattern metadata.
/// Used for calibration frame processing (darks, flats, bias).
pub fn load_raw_cfa(path: &Path) -> Result<CfaImage>
```

Implementation approach:
- Same libraw init/open/unpack as `load_raw()`
- Same sensor detection via `detect_sensor_type()`
- For **Monochrome**: same as current `process_monochrome()`, return as `CfaImage` with no CFA type (or just return as grayscale since mono has no CFA issue - could make `CfaType` optional or add a `Mono` variant)
- For **Bayer**: extract raw u16 data, normalize to f32, crop to active area → return 1-channel `CfaImage` with `CfaType::Bayer(pattern)`
- For **X-Trans**: extract raw u16 data, normalize to f32, crop to active area → return 1-channel `CfaImage` with `CfaType::XTrans(pattern_6x6)`
- For **Unknown**: fall back to `load_raw()` (demosaic with libraw, return as AstroImage). Cannot support raw CFA for unknown patterns.

The Bayer and X-Trans branches reuse the existing normalization (`normalize_u16_to_f32_parallel`) and active-area cropping logic from `process_monochrome()`. The only difference vs current code: skip the demosaic step.

### Step 3: Implement `CfaImage::demosaic()`

**File:** `lumos/src/astro_image/cfa.rs`

```rust
impl CfaImage {
    pub fn demosaic(self) -> AstroImage {
        match &self.pattern {
            CfaType::Bayer(cfa_pattern) => {
                // Build BayerImage from self.pixels (already f32, no margins)
                let bayer = BayerImage::with_margins(
                    &self.pixels,
                    self.width, self.height,  // raw = active (no margins after crop)
                    self.width, self.height,
                    0, 0,                     // no margins
                    *cfa_pattern,
                );
                let rgb = demosaic_bayer(&bayer);
                let dims = ImageDimensions::new(self.width, self.height, 3);
                let mut astro = AstroImage::from_pixels(dims, rgb);
                astro.metadata = self.metadata;
                astro
            }
            CfaType::XTrans(pattern) => {
                // Convert f32 back to u16 for process_xtrans (it expects u16 + normalization params)
                // OR: add a new xtrans demosaic entry point that takes f32 data directly.
                // The second option is cleaner. Add process_xtrans_f32() that takes &[f32].
                ...
            }
        }
    }
}
```

**X-Trans consideration:** The current `process_xtrans()` takes `&[u16]` + black/inv_range because it normalizes on-the-fly to save memory. For `CfaImage::demosaic()`, data is already f32-normalized. Two options:

- **Option 1:** Add `process_xtrans_f32()` that takes `&[f32]` directly. The Markesteijn algorithm internally uses f32 anyway, so this just skips the u16→f32 conversion.
- **Option 2:** Convert f32 back to u16 (lossy, wasteful). Not recommended.

Go with Option 1: add a `demosaic_xtrans_markesteijn_f32()` variant in `xtrans/markesteijn.rs` that accepts pre-normalized f32 data. The XTransImage struct needs a parallel version or a trait abstraction. Simplest approach: make `XTransImage` generic or add a `read_normalized` that just indexes into f32 data directly.

### Step 4: Rewrite `HotPixelMap` for raw CFA data

**File:** `lumos/src/stacking/hot_pixels.rs`

Remove `HotPixelMask` enum (no more L/Rgb distinction). The struct becomes:

```rust
#[derive(Debug, Clone)]
pub struct HotPixelMap {
    /// Sorted flat pixel indices of hot pixels.
    indices: Vec<usize>,
    pub width: usize,
    pub height: usize,
}
```

**Detection** (`from_master_dark` → `from_cfa_dark`):

```rust
impl HotPixelMap {
    /// Detect hot pixels from a raw CFA master dark.
    pub fn from_cfa_dark(dark: &CfaImage, sigma_threshold: f32) -> Self {
        // Same MAD algorithm as before, but on single-channel data.
        // No per-channel logic needed - just one channel.
        let median = sampled_median(&dark.pixels);
        let mad = sampled_mad(&dark.pixels, median);
        let sigma = (mad * 1.4826).max(median * 0.1);
        let threshold = median + sigma_threshold * sigma;

        let indices: Vec<usize> = dark.pixels.iter()
            .enumerate()
            .filter(|(_, &v)| v > threshold)
            .map(|(i, _)| i)
            .collect();

        Self { indices, width: dark.width, height: dark.height }
    }
}
```

**CFA-aware correction** (`correct` → `correct_cfa`):

```rust
impl HotPixelMap {
    /// Replace hot pixels with median of same-color CFA neighbors.
    pub fn correct_cfa(&self, image: &mut CfaImage) {
        assert!(image.width == self.width && image.height == self.height);

        for &idx in &self.indices {
            let x = idx % self.width;
            let y = idx / self.width;
            image.pixels[idx] = median_same_color_neighbors(
                &image.pixels,
                self.width,
                self.height,
                x, y,
                &image.pattern,
            );
        }
    }
}
```

**Same-color neighbor median:**

```rust
/// Find same-color neighbors and return their median.
fn median_same_color_neighbors(
    pixels: &[f32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    pattern: &CfaType,
) -> f32 {
    let my_color = pattern.color_at(x, y);
    let mut neighbors = ArrayVec::<f32, 24>::new(); // max neighbors we'd ever collect

    let period = pattern.period();
    // Search within a radius that guarantees finding enough same-color neighbors.
    // Bayer (period=2): search radius 4 gives up to 8 same-color neighbors.
    // X-Trans (period=6): search radius 6 gives enough same-color neighbors.
    let radius = period as i32 * 2;

    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx == 0 && dy == 0 { continue; }
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 { continue; }
            let nx = nx as usize;
            let ny = ny as usize;
            if pattern.color_at(nx, ny) == my_color {
                neighbors.push(pixels[ny * width + nx]);
            }
        }
    }

    median_f32_mut(&mut neighbors)
}
```

For Bayer specifically, this generic search finds:
- R/B pixels at (x,y): same-color neighbors at offsets (+-2, 0), (0, +-2), (+-2, +-2) → up to 8 neighbors
- G pixels at (x,y): same-color neighbors at offsets (+-1, +-1), (+-2, 0), (0, +-2) → up to 8 neighbors

This is correct and matches what Siril does (distance-2 sampling for Bayer).

For X-Trans, the generic search with radius=12 finds all same-color pixels within the surrounding 25x25 grid - more than enough for a robust median.

**Optimization:** For Bayer, precompute the 8 same-color neighbor offsets at compile time instead of the generic search. The generic search is fine for correctness but adds overhead scanning the full radius grid. A specialized Bayer path would be:

```rust
fn bayer_same_color_neighbors(pixels: &[f32], w: usize, h: usize, x: usize, y: usize) -> f32 {
    // Same-color neighbors are always at stride 2 (period of Bayer pattern)
    let offsets: [(i32, i32); 8] = [
        (-2, 0), (2, 0), (0, -2), (0, 2),
        (-2, -2), (-2, 2), (2, -2), (2, 2),
    ];
    let mut buf = [0.0f32; 8];
    let mut count = 0;
    for (dx, dy) in offsets {
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;
        if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
            buf[count] = pixels[ny as usize * w + nx as usize];
            count += 1;
        }
    }
    median_f32_mut(&mut buf[..count])
}
```

For X-Trans, build a lookup table of same-color offsets for each of the 36 positions in the 6x6 tile, computed once at startup:

```rust
fn build_xtrans_neighbor_table(pattern: &[[u8; 6]; 6]) -> [Vec<(i32, i32)>; 36] {
    let mut table = std::array::from_fn(|_| Vec::new());
    for y in 0..6 {
        for x in 0..6 {
            let my_color = pattern[y][x];
            let offsets = &mut table[y * 6 + x];
            // Search within 6 pixels (one full period)
            for dy in -6i32..=6 {
                for dx in -6i32..=6 {
                    if dx == 0 && dy == 0 { continue; }
                    let ny = ((y as i32 + dy).rem_euclid(6)) as usize;
                    let nx = ((x as i32 + dx).rem_euclid(6)) as usize;
                    if pattern[ny][nx] == my_color {
                        offsets.push((dx, dy));
                    }
                }
            }
            // Sort by Manhattan distance for consistent ordering
            offsets.sort_by_key(|(dx, dy)| dx.abs() + dy.abs());
            // Keep only the closest ~12 neighbors
            offsets.truncate(12);
        }
    }
    table
}
```

### Step 5: Implement `CfaImage` calibration methods

**File:** `lumos/src/astro_image/cfa.rs`

```rust
impl CfaImage {
    /// Subtract dark frame pixel-by-pixel. Both must have same dimensions.
    pub fn subtract(&mut self, dark: &CfaImage) {
        assert!(self.width == dark.width && self.height == dark.height);
        // CFA alignment is guaranteed (same camera/sensor).
        // Simple element-wise subtraction.
        self.pixels.par_iter_mut()
            .zip(dark.pixels.par_iter())
            .for_each(|(l, d)| *l -= d);
    }

    /// Divide by normalized flat (with optional bias subtraction from flat).
    /// Formula: light /= (flat - bias) / mean(flat - bias)
    pub fn divide_by_normalized(&mut self, flat: &CfaImage, bias: Option<&CfaImage>) {
        assert!(self.width == flat.width && self.height == flat.height);

        let flat_mean = if let Some(bias) = bias {
            assert!(bias.width == flat.width && bias.height == flat.height);
            // mean(flat - bias)
            let sum: f32 = flat.pixels.par_iter()
                .zip(bias.pixels.par_iter())
                .map(|(f, b)| f - b)
                .sum();
            sum / flat.pixels.len() as f32
        } else {
            let sum: f32 = flat.pixels.par_iter().sum();
            sum / flat.pixels.len() as f32
        };

        assert!(flat_mean > f32::EPSILON);
        let inv_mean = 1.0 / flat_mean;

        match bias {
            Some(bias) => {
                self.pixels.par_iter_mut()
                    .zip(flat.pixels.par_iter().zip(bias.pixels.par_iter()))
                    .for_each(|(l, (f, b))| {
                        let norm_flat = (f - b) * inv_mean;
                        if norm_flat > f32::EPSILON {
                            *l /= norm_flat;
                        }
                    });
            }
            None => {
                self.pixels.par_iter_mut()
                    .zip(flat.pixels.par_iter())
                    .for_each(|(l, f)| {
                        let norm_flat = f * inv_mean;
                        if norm_flat > f32::EPSILON {
                            *l /= norm_flat;
                        }
                    });
            }
        }
    }
}
```

### Step 6: Rewrite `CalibrationMasters` for raw CFA pipeline

**File:** `lumos/src/calibration_masters/mod.rs`

The core change: masters are `CfaImage` (1 channel) not `AstroImage` (3 channel RGB).

```rust
pub struct CalibrationMasters {
    pub master_dark_raw: Option<CfaImage>,
    pub master_flat_raw: Option<CfaImage>,
    pub master_bias_raw: Option<CfaImage>,
    pub hot_pixel_map: Option<HotPixelMap>,
    pub config: StackConfig,
}
```

**`calibrate()` method changes:**

Old signature: `pub fn calibrate(&self, image: &mut AstroImage)`
New signature: `pub fn calibrate_raw(&self, image: &mut CfaImage)`

```rust
pub fn calibrate_raw(&self, image: &mut CfaImage) {
    // 1. Hot pixel correction on raw CFA (before dark subtraction - corrects raw defects)
    if let Some(ref hot_map) = self.hot_pixel_map {
        hot_map.correct_cfa(image);
    }

    // 2. Dark subtraction on raw CFA
    if let Some(ref dark) = self.master_dark_raw {
        image.subtract(dark);
    } else if let Some(ref bias) = self.master_bias_raw {
        image.subtract(bias);
    }

    // 3. Flat division on raw CFA
    if let Some(ref flat) = self.master_flat_raw {
        image.divide_by_normalized(flat, self.master_bias_raw.as_ref());
    }
}
```

Note: hot pixel correction is applied **first**, before dark subtraction. This is because we detect hot pixels from the master dark (which has the full bias+thermal signal), and the light frame also has full signal at this point. If we subtracted dark first, the hot pixel values would be near-zero (signal canceled) and couldn't be reliably identified by position alone. Since we use a position-based map (not threshold-based on the light), the order is actually: correct → subtract → divide.

Actually, since we're using a precomputed position map (not re-detecting on the light), the order doesn't matter for detection. The correction replaces the hot pixel value with the median of neighbors regardless of signal level. So the order could be:
1. subtract dark
2. divide flat
3. correct hot pixels

Or:
1. correct hot pixels
2. subtract dark
3. divide flat

Both are valid since we use a fixed position map. The standard convention (PixInsight) is to correct after calibration, but before demosaic. Let's follow that:

```rust
pub fn calibrate_raw(&self, image: &mut CfaImage) {
    // 1. Dark subtraction
    if let Some(ref dark) = self.master_dark_raw {
        image.subtract(dark);
    } else if let Some(ref bias) = self.master_bias_raw {
        image.subtract(bias);
    }

    // 2. Flat division
    if let Some(ref flat) = self.master_flat_raw {
        image.divide_by_normalized(flat, self.master_bias_raw.as_ref());
    }

    // 3. Hot pixel correction (CFA-aware, before demosaic)
    if let Some(ref hot_map) = self.hot_pixel_map {
        hot_map.correct_cfa(image);
    }
}
```

**`create()` changes:**

```rust
fn create_master_cfa(
    base_dir: &Path,
    subdir: &str,
    config: &StackConfig,
    progress: &ProgressCallback,
) -> Option<CfaImage> {
    let dir = base_dir.join(subdir);
    if !dir.exists() { return None; }

    let paths = common::file_utils::astro_image_files(&dir);
    if paths.is_empty() { return None; }

    // Load as CfaImage (1 channel), stack, return CfaImage
    stack_cfa_with_progress(&paths, config.clone(), progress.clone()).ok()
}
```

### Step 7: Add CFA stacking support

**File:** `lumos/src/stacking/stack.rs` (or new file `lumos/src/stacking/cfa_stack.rs`)

The existing `ImageCache` and `process_chunked()` already work with 1-channel grayscale data. The key change is loading images as `CfaImage` instead of `AstroImage`.

Two approaches:

**Approach A - Reuse existing ImageCache:** Since `CfaImage` has a single channel of f32 data, convert it to a 1-channel `AstroImage` for stacking purposes, then convert the stacked result back to `CfaImage`:

```rust
pub fn stack_cfa_with_progress(
    paths: &[PathBuf],
    config: StackConfig,
    progress: ProgressCallback,
) -> Result<CfaImage> {
    // Load first frame to get CFA pattern
    let first = load_raw_cfa(&paths[0])?;
    let pattern = first.pattern.clone();
    let metadata = first.metadata.clone();

    // Convert CfaImage → 1-channel AstroImage for stacking
    // (ImageCache works with AstroImage)
    // Then convert result back to CfaImage

    // Use existing stack infrastructure with a custom loader
    ...
}
```

**Approach B - Extend ImageCache to accept CfaImage:** Add a `from_cfa_paths()` constructor that loads `CfaImage` and stores as 1-channel data. The `process_chunked()` logic doesn't change - it already handles 1-channel data.

Approach A is simpler and requires fewer changes to the stacking infrastructure. The conversion is zero-cost: `CfaImage.pixels` becomes `PixelData::L(Buffer2::new(w, h, pixels))`.

### Step 8: Save/Load raw masters as TIFF

**File:** `lumos/src/calibration_masters/mod.rs`

Raw CFA masters are saved as single-channel f32 TIFF (grayscale), same as before but 1 channel instead of 3. The CFA pattern metadata needs to be stored separately (the TIFF itself doesn't know about CFA patterns).

Options for persisting CFA pattern:
1. **Sidecar file:** Save `master_dark_median.cfa` with pattern info (JSON or binary).
2. **TIFF metadata:** Store CFA pattern in TIFF tags (non-standard but works).
3. **Re-detect:** When loading a master dark, require the user to also provide a sample raw frame to extract the CFA pattern from.
4. **Convention:** Assume the CFA pattern is the same for all frames from the same camera. Store pattern once alongside masters.

**Recommendation:** Sidecar JSON file. When saving masters, write `cfa_pattern.json`:

```json
{
  "type": "Bayer",
  "pattern": "Rggb"
}
```

or for X-Trans:

```json
{
  "type": "XTrans",
  "pattern": [[0,1,0,0,2,0],[2,1,2,1,1,1],[0,1,0,0,0,1],[0,2,0,0,0,1],[1,0,1,2,0,2],[1,0,0,0,0,1]]
}
```

### Step 9: Update `full_pipeline.rs` example

**File:** `lumos/examples/full_pipeline.rs`

The pipeline changes from:

```
load_raw → (demosaiced AstroImage) → calibrate → star detect → register → stack
```

To:

```
load_raw_cfa → (CfaImage) → calibrate_raw → demosaic → (AstroImage) → star detect → register → stack
```

Step 2 (`calibrate_light_frames`) becomes:

```rust
fn calibrate_light_frames(...) {
    for path in &light_paths {
        let mut cfa = load_raw_cfa(path)?;   // 1-channel CFA
        masters.calibrate_raw(&mut cfa);       // hot pixel + dark + flat on raw
        let light: AstroImage = cfa.demosaic(); // demosaic to RGB
        // save calibrated RGB light
    }
}
```

### Step 10: Update `AstroImage::from_file()` routing

**File:** `lumos/src/astro_image/mod.rs`

Keep `from_file()` unchanged - it still returns demosaiced `AstroImage` (for non-calibration use cases like star detection on already-calibrated TIFF files).

Add `from_file_cfa()` for the calibration path:

```rust
impl CfaImage {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let ext = path.extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "raf" | "cr2" | "cr3" | "nef" | "arw" | "dng" => load_raw_cfa(path),
            _ => anyhow::bail!("CFA loading only supported for RAW formats, got .{}", ext),
        }
    }
}
```

### Step 11: Handle monochrome and unknown sensors

**Monochrome sensors:** No CFA to worry about. `load_raw_cfa()` could return `CfaImage` with a `CfaType::Mono` variant, or just return `AstroImage` directly. Since the whole point of `CfaImage` is CFA-aware processing, monochrome should bypass this entirely and use the existing `AstroImage` pipeline. Add `CfaType::Mono` to handle this gracefully:

```rust
pub enum CfaType {
    Mono,               // No CFA pattern
    Bayer(CfaPattern),
    XTrans([[u8; 6]; 6]),
}
```

For `Mono`, `demosaic()` is a no-op (convert 1-channel CfaImage to 1-channel AstroImage). Hot pixel correction uses direct 8-neighbor median (same as current).

**Unknown sensors:** Cannot extract raw CFA data. Fall back to the current pipeline (libraw demosaic → AstroImage). Document this limitation.

## File Change Summary

| File | Change |
|------|--------|
| `lumos/src/astro_image/cfa.rs` | **NEW** - CfaImage, CfaType, demosaic, calibration ops |
| `lumos/src/astro_image/mod.rs` | Add `pub mod cfa;` |
| `lumos/src/raw/mod.rs` | Add `load_raw_cfa()` function |
| `lumos/src/raw/demosaic/bayer/mod.rs` | Make `CfaPattern` public (currently `pub(crate)`) |
| `lumos/src/raw/demosaic/xtrans/mod.rs` | Add f32-input demosaic variant |
| `lumos/src/stacking/hot_pixels.rs` | Rewrite for position-based map + CFA-aware correction |
| `lumos/src/calibration_masters/mod.rs` | Rewrite for CfaImage masters |
| `lumos/src/lib.rs` | Export CfaImage, CfaType |
| `lumos/examples/full_pipeline.rs` | Update to use CFA pipeline |

## Backward Compatibility

**No backward compatibility is maintained** (per CLAUDE.md rules). The old demosaiced-masters pipeline is removed. Existing saved master TIFF files (3-channel RGB) are incompatible and must be regenerated.

## Testing

1. **Unit tests for CFA-aware neighbor finding:** Verify that `median_same_color_neighbors()` returns correct neighbors for all Bayer patterns (RGGB, BGGR, GRBG, GBRG) and X-Trans positions.
2. **Unit tests for hot pixel detection on raw data:** Synthetic 1-channel image with known hot pixels, verify all detected.
3. **Unit tests for CfaImage calibration ops:** `subtract()`, `divide_by_normalized()` on synthetic data.
4. **Integration test:** Load real RAW dark frames → stack as CFA → detect hot pixels → verify detection on raw data produces a reasonable count similar to (or better than) current RGB detection.
5. **Round-trip test:** `load_raw_cfa()` → `demosaic()` should produce the same result as `load_raw()` for the same input file.
