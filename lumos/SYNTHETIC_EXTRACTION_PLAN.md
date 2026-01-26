# Synthetic Data Generation Extraction Plan

## Goal
Extract all synthetic data generation code from `star_detection` and `registration` modules into a centralized `testing/synthetic` module with convenient APIs.

## Current State

### Star Detection - Visual Tests Generators
Location: `src/star_detection/visual_tests/generators/`
- `star_field.rs` - Main star field generator with `StarFieldConfig`
- `star_profiles.rs` - PSF rendering (Gaussian, Moffat, elliptical, saturated)
- `backgrounds.rs` - Background generators (uniform, gradient, vignette, nebula)
- `artifacts.rs` - Artifact generators (cosmic rays, hot pixels, bad columns, Bayer)
- `mod.rs` - Module exports

### Star Detection - Legacy Synthetic
Location: `src/star_detection/visual_tests/synthetic.rs`
- Simple `SyntheticStar` and `SyntheticFieldConfig`
- Basic `generate_star_field()` function

### Registration - Synthetic Tests
Location: `src/registration/tests/synthetic_registration.rs`
- `generate_star_field()` - Random star positions
- `translate_stars()`, `transform_stars()` - Transform application

## Target Structure

```
src/testing/
├── mod.rs                    # Existing + re-exports synthetic module
└── synthetic/
    ├── mod.rs                # Main API and re-exports
    ├── star_field.rs         # Star field generation
    ├── star_profiles.rs      # PSF rendering functions
    ├── backgrounds.rs        # Background generators
    ├── artifacts.rs          # Artifact generators
    ├── transforms.rs         # Star position transforms (from registration)
    └── presets.rs            # Convenient preset configurations
```

## New Convenient API

```rust
// Quick generation with presets
let (pixels, stars) = synthetic::sparse_field(512, 512);
let (pixels, stars) = synthetic::dense_field(1024, 1024);
let (pixels, stars) = synthetic::crowded_cluster(512, 512);

// Custom generation with builder
let field = synthetic::StarFieldBuilder::new(1024, 1024)
    .star_count(100)
    .fwhm(4.0)
    .noise_sigma(10.0)
    .background(500.0)
    .seed(42)
    .build();

// Transform star positions (for registration tests)
let translated = synthetic::translate_stars(&stars, 25.0, -15.0);
let transformed = synthetic::transform_stars(&stars, &transform);

// Individual components
synthetic::render_gaussian_star(&mut pixels, width, x, y, flux, sigma);
synthetic::add_cosmic_rays(&mut pixels, width, height, count, &mut rng);
synthetic::add_gradient_background(&mut pixels, width, height, angle, strength);
```

## Implementation Steps

### Phase 1: Create Module Structure
1. Create `src/testing/synthetic/` directory
2. Create `mod.rs` with module structure
3. Move files from `star_detection/visual_tests/generators/`:
   - `star_field.rs`
   - `star_profiles.rs`
   - `backgrounds.rs`
   - `artifacts.rs`

### Phase 2: Add Transform Utilities
4. Create `transforms.rs` with functions from `registration/tests/synthetic_registration.rs`:
   - `generate_random_positions()`
   - `translate_stars()`
   - `transform_stars()`

### Phase 3: Create Convenient Presets
5. Create `presets.rs` with quick-access functions:
   - `sparse_field(width, height)` - 20 well-separated stars
   - `dense_field(width, height)` - 200 stars with crowding
   - `crowded_cluster(width, height)` - 500 stars clustered
   - `faint_stars(width, height)` - Faint stars near detection limit
   - `elliptical_stars(width, height)` - Stars with tracking errors

### Phase 4: Add Builder Pattern
6. Create `StarFieldBuilder` for flexible configuration:
   - Chainable methods for all parameters
   - Sensible defaults
   - `build()` returns `(Vec<f32>, Vec<GroundTruthStar>)`

### Phase 5: Update Imports
7. Update `star_detection/visual_tests/mod.rs` to use new location
8. Update `star_detection/visual_tests/pipeline_tests/` to use new location
9. Update `registration/tests/synthetic_registration.rs` to use new location
10. Remove old `star_detection/visual_tests/synthetic.rs` (merge into new module)
11. Remove old `star_detection/visual_tests/generators/` directory

### Phase 6: Update Testing Module
12. Update `src/testing/mod.rs` to export synthetic module
13. Add convenient re-exports at crate level if needed

## Files to Move/Modify

### Move (copy content, then delete original):
- `star_detection/visual_tests/generators/star_field.rs` → `testing/synthetic/star_field.rs`
- `star_detection/visual_tests/generators/star_profiles.rs` → `testing/synthetic/star_profiles.rs`
- `star_detection/visual_tests/generators/backgrounds.rs` → `testing/synthetic/backgrounds.rs`
- `star_detection/visual_tests/generators/artifacts.rs` → `testing/synthetic/artifacts.rs`

### Extract and merge:
- `registration/tests/synthetic_registration.rs` (transform functions) → `testing/synthetic/transforms.rs`
- `star_detection/visual_tests/synthetic.rs` (simple API) → merge into presets

### Update imports in:
- `star_detection/visual_tests/mod.rs`
- `star_detection/visual_tests/pipeline_tests/standard_tests.rs`
- `star_detection/visual_tests/debug_steps.rs`
- `star_detection/visual_tests/subpixel_accuracy.rs`
- `registration/tests/synthetic_registration.rs`
- `registration/triangle/tests.rs` (if applicable)

### Delete after migration:
- `star_detection/visual_tests/generators/` (entire directory)
- `star_detection/visual_tests/synthetic.rs`

## Testing
- Run all tests after each phase to ensure nothing breaks
- `cargo test -p lumos --lib`
- `cargo clippy --all-targets -- -D warnings`
