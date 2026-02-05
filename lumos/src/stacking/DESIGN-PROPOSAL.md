# Stacking Module API Design Proposal

## Current State Analysis

### Issues Identified

1. **Duplicate Type Definition**: `SigmaClipConfig` is defined in both `mod.rs` and `rejection.rs` with identical implementations, causing import ambiguity.

2. **Inconsistent Configuration Patterns**:
   - `MedianConfig` is a type alias for `CacheConfig` (non-obvious)
   - `SigmaClippedConfig` wraps `SigmaClipConfig` + `CacheConfig`
   - `WeightedConfig` has different structure (weights + rejection + cache)
   - Some configs have constructors, others don't

3. **Validation Timing Inconsistency**:
   - Most configs validate at construction (`SigmaClipConfig::new` asserts sigma > 0)
   - `WeightedConfig` validates weight count at runtime during stacking (panics if mismatch)

4. **No Builder Pattern**: Complex configs require setting multiple fields manually without fluent API.

5. **Asymmetric Module Structure**:
   - `weighted/mod.rs` has full implementation
   - `median/mod.rs` and `sigma_clipped/mod.rs` are thin wrappers
   - `mean/mod.rs` is minimal

6. **Unused Public API**: Local normalization types marked `#[allow(dead_code)]` despite being fully implemented.

7. **Cache Cleanup Inconsistency**: Only `SigmaClippedConfig` explicitly cleans up cache.

---

## Industry Best Practices Review

### Astropy ccdproc ([API Reference](https://ccdproc.readthedocs.io/en/latest/api/ccdproc.combine.html))

```python
ccdproc.combine(
    img_list,
    method='average',           # Combination method
    weights=None,               # Per-image weights
    scale=None,                 # Scaling function/array
    sigma_clip=False,           # Enable/disable
    sigma_clip_low_thresh=3,    # Asymmetric thresholds
    sigma_clip_high_thresh=3,
    clip_extrema=False,         # Alternative clipping
    nlow=1, nhigh=1,            # Extrema counts
    mem_limit=16e9,             # Memory management
)
```

**Key patterns**:
- Single entry point with all options
- Boolean flags to enable features
- Asymmetric sigma thresholds by default
- Memory limit as explicit parameter

### Siril ([Documentation](https://siril.readthedocs.io/en/stable/preprocessing/stacking.html))

**Stacking Methods**: Sum, Average+Rejection, Median, Max, Min

**Rejection Algorithms**: Percentile, Sigma, MAD, Median Sigma, Winsorized, GESD, Linear Fit

**Normalization**: None, Additive, Multiplicative, Additive+Scaling, Multiplicative+Scaling

**Weighting**: Star count, FWHM, Noise, Integration time

**Key pattern**: Explicit normalization as separate concept from rejection.

### Rust Builder Pattern ([Best Practices](https://rust-unofficial.github.io/patterns/patterns/creational/builder.html))

```rust
// Recommended fluent builder
let config = StackConfig::builder()
    .method(Method::SigmaClip)
    .sigma(2.5)
    .iterations(3)
    .build();
```

**Key patterns**:
- `builder()` method on target struct
- Fluent API with method chaining
- `build()` consumes builder and validates
- Use `Default` for optional fields

---

## Proposed Design

### Principle 1: Single Unified Configuration

Replace multiple config types with one `StackConfig`:

```rust
/// Configuration for image stacking operations.
#[derive(Debug, Clone)]
pub struct StackConfig {
    /// How to combine pixel values across frames.
    pub method: CombineMethod,
    
    /// Pixel rejection to remove outliers before combining.
    pub rejection: Rejection,
    
    /// Per-frame weights (empty = equal weights).
    pub weights: Vec<f32>,
    
    /// Frame normalization before stacking.
    pub normalization: Normalization,
    
    /// Memory/caching behavior.
    pub memory: MemoryConfig,
}

impl Default for StackConfig {
    fn default() -> Self {
        Self {
            method: CombineMethod::Mean,
            rejection: Rejection::SigmaClip { sigma: 2.5, iterations: 3 },
            weights: vec![],
            normalization: Normalization::None,
            memory: MemoryConfig::default(),
        }
    }
}
```

### Principle 2: Flat Enum Variants

Use enums with inline parameters instead of nested config structs:

```rust
/// Method for combining pixel values.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CombineMethod {
    /// Simple average (no rejection, fastest).
    #[default]
    Mean,
    /// Median value (implicit outlier rejection).
    Median,
    /// Weighted mean using per-frame weights.
    WeightedMean,
    /// Sum all values (for star trails).
    Sum,
}

/// Pixel rejection algorithm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rejection {
    /// No rejection.
    None,
    /// Iterative sigma clipping from median.
    SigmaClip {
        sigma: f32,
        iterations: u32,
    },
    /// Sigma clipping with asymmetric thresholds.
    SigmaClipAsymmetric {
        sigma_low: f32,
        sigma_high: f32,
        iterations: u32,
    },
    /// Replace outliers with boundary values (better for small stacks).
    Winsorized {
        sigma: f32,
        iterations: u32,
    },
    /// Fit linear trend, reject deviants (good for gradients).
    LinearFit {
        sigma_low: f32,
        sigma_high: f32,
        iterations: u32,
    },
    /// Clip lowest/highest percentiles.
    Percentile {
        low: f32,
        high: f32,
    },
    /// Generalized ESD test (best for large stacks).
    Gesd {
        alpha: f32,
        max_outliers: Option<usize>,
    },
}

impl Default for Rejection {
    fn default() -> Self {
        Self::SigmaClip { sigma: 2.5, iterations: 3 }
    }
}

/// Frame normalization method.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Normalization {
    /// No normalization.
    #[default]
    None,
    /// Match global median and scale.
    Global,
    /// Tile-based local normalization.
    Local {
        tile_size: usize,
    },
}

/// Memory and caching configuration.
#[derive(Debug, Clone)]
pub struct MemoryConfig {
    /// Maximum memory to use (None = auto-detect).
    pub limit: Option<u64>,
    /// Cache directory for disk-backed processing.
    pub cache_dir: Option<PathBuf>,
    /// Keep cache files after completion.
    pub keep_cache: bool,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            limit: None,
            cache_dir: None,
            keep_cache: false,
        }
    }
}
```

### Principle 3: Constructor Presets

```rust
impl StackConfig {
    /// Quick preset: sigma-clipped mean (most common).
    pub fn sigma_clipped(sigma: f32) -> Self {
        Self {
            rejection: Rejection::SigmaClip { sigma, iterations: 3 },
            ..Default::default()
        }
    }
    
    /// Quick preset: median stacking.
    pub fn median() -> Self {
        Self {
            method: CombineMethod::Median,
            rejection: Rejection::None,
            ..Default::default()
        }
    }
    
    /// Quick preset: weighted mean with quality-based weights.
    pub fn weighted(weights: Vec<f32>) -> Self {
        Self {
            method: CombineMethod::WeightedMean,
            weights,
            ..Default::default()
        }
    }
    
    /// Quick preset: weighted mean from quality metrics.
    pub fn weighted_from_quality(qualities: &[FrameQuality]) -> Self {
        Self {
            method: CombineMethod::WeightedMean,
            weights: qualities.iter().map(|q| q.weight()).collect(),
            ..Default::default()
        }
    }
}
```

### Principle 4: Simplified Entry Point

```rust
/// Stack multiple images into a single result.
///
/// # Example
///
/// ```rust
/// use lumos::stacking::{stack, StackConfig, FrameType};
///
/// // Simple sigma-clipped stacking (default)
/// let result = stack(&paths, FrameType::Light, StackConfig::default())?;
///
/// // Using presets
/// let result = stack(&paths, FrameType::Light, StackConfig::sigma_clipped(2.0))?;
/// let result = stack(&paths, FrameType::Light, StackConfig::median())?;
///
/// // Custom configuration using struct update syntax
/// let config = StackConfig {
///     rejection: Rejection::SigmaClipAsymmetric {
///         sigma_low: 2.0,
///         sigma_high: 3.0,
///         iterations: 5,
///     },
///     normalization: Normalization::Local { tile_size: 128 },
///     ..Default::default()
/// };
/// let result = stack(&paths, FrameType::Light, config)?;
///
/// // With quality-based weights
/// let config = StackConfig::weighted_from_quality(&qualities);
/// let result = stack(&paths, FrameType::Light, config)?;
/// ```
pub fn stack<P: AsRef<Path>>(
    paths: &[P],
    frame_type: FrameType,
    config: StackConfig,
) -> Result<AstroImage, Error> {
    stack_with_progress(paths, frame_type, config, ProgressCallback::default())
}

pub fn stack_with_progress<P: AsRef<Path>>(
    paths: &[P],
    frame_type: FrameType,
    config: StackConfig,
    progress: ProgressCallback,
) -> Result<AstroImage, Error> {
    // Implementation dispatches based on config
}
```

---

## API Comparison

### Current API

```rust
// Complex, multiple config types
let cache = CacheConfig {
    cache_dir: PathBuf::from("/tmp"),
    keep_cache: false,
    available_memory: None,
};
let clip = SigmaClipConfig::new(2.5, 3);
let config = SigmaClippedConfig { clip, cache };
let stack = ImageStack::new(FrameType::Light, StackingMethod::SigmaClippedMean(config), paths);
let result = stack.process(ProgressCallback::default())?;

// Weighted requires different pattern
let config = WeightedConfig {
    weights: qualities.iter().map(|q| q.compute_weight()).collect(),
    rejection: RejectionMethod::SigmaClip(SigmaClipConfig::new(2.5, 3)),
    cache: CacheConfig::default(),
};
let stack = ImageStack::new(FrameType::Light, StackingMethod::WeightedMean(config), paths);
```

### Proposed API

```rust
// Simple default
let result = stack(&paths, FrameType::Light, StackConfig::default())?;

// Quick presets
let result = stack(&paths, FrameType::Light, StackConfig::sigma_clipped(2.5))?;
let result = stack(&paths, FrameType::Light, StackConfig::median())?;

// With quality-based weights
let result = stack(&paths, FrameType::Light, StackConfig::weighted_from_quality(&qualities))?;

// Custom config using struct update syntax
let config = StackConfig {
    rejection: Rejection::SigmaClipAsymmetric {
        sigma_low: 2.0,
        sigma_high: 3.0,
        iterations: 5,
    },
    normalization: Normalization::Local { tile_size: 128 },
    memory: MemoryConfig {
        limit: Some(8 * 1024 * 1024 * 1024),
        ..Default::default()
    },
    ..Default::default()
};
let result = stack(&paths, FrameType::Light, config)?;
```

---

## Migration Path

1. **Phase 1**: Add new unified API alongside existing
   - Add `StackConfig`, builder, and `stack()` function
   - Keep old API working (deprecated)

2. **Phase 2**: Migrate internal code
   - Update all internal usage to new API
   - Ensure tests pass

3. **Phase 3**: Remove old API
   - Remove `ImageStack` struct
   - Remove duplicate `SigmaClipConfig`
   - Remove type aliases like `MedianConfig`

---

## Summary of Changes

| Aspect | Current | Proposed |
|--------|---------|----------|
| Entry point | `ImageStack::new().process()` | `stack()` function |
| Config types | 6+ separate structs | 1 unified `StackConfig` |
| Configuration | Manual struct construction | Presets + struct update syntax |
| Rejection config | Nested structs | Flat enum variants |
| Asymmetric sigma | Only in LinearFit | First-class `SigmaClipAsymmetric` |
| Normalization | Separate module, unused | Integrated in config |
| Memory config | Part of method config | Separate `MemoryConfig` |
| Presets | None | `sigma_clipped()`, `median()`, `weighted()` |

---

## References

- [Astropy ccdproc API](https://ccdproc.readthedocs.io/en/latest/api/ccdproc.combine.html)
- [Siril Stacking Documentation](https://siril.readthedocs.io/en/stable/preprocessing/stacking.html)
- [Rust Builder Pattern](https://rust-unofficial.github.io/patterns/patterns/creational/builder.html)
- [Nine Rules for Elegant Rust APIs](https://towardsdatascience.com/nine-rules-for-elegant-rust-library-apis-9b986a465247/)
- [PixInsight ImageIntegration](https://chaoticnebula.com/pixinsight-image-integration/)
