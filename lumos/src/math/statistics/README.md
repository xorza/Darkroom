# Statistics Module

Statistical functions for robust estimation: median, MAD, and sigma-clipped statistics.

## Functions

| Function | Description | Complexity |
|----------|-------------|------------|
| `median_f32_mut` | Median via quickselect | O(n) average |
| `median_and_mad_f32_mut` | Median and MAD together | O(n) |
| `mad_f32_with_scratch` | MAD with external scratch buffer | O(n) |
| `sigma_clipped_median_mad` | Iterative outlier rejection (Vec) | O(n × iterations) |
| `sigma_clipped_median_mad_arrayvec` | Same, with stack-allocated buffer | O(n × iterations) |
| `mad_to_sigma` | Convert MAD to standard deviation | O(1) |

## Implementation Details

### Median Computation

Uses `select_nth_unstable_by` (quickselect algorithm):
- **O(n) average** time complexity vs O(n log n) for full sort
- In-place partitioning, no extra allocation
- For even-length arrays, computes average of two middle elements

### Why No SIMD for Median?

Quickselect has **data-dependent branching** that cannot be vectorized:
- Pivot selection and partitioning depend on comparisons
- Memory access patterns are irregular
- SIMD would require different algorithm (sorting networks) with worse complexity

### Sigma-Clipped Statistics

Iteratively rejects outliers beyond `kappa × sigma` from the median:

```
for each iteration:
    1. Compute median (quickselect)
    2. Compute absolute deviations
    3. Compute MAD (median of deviations)
    4. Convert MAD to sigma (× 1.4826)
    5. Reject values outside [median ± kappa×sigma]
    6. Stop if converged (no rejections)
```

### Vec vs ArrayVec Variants

Two versions exist for different allocation strategies:

| Variant | Allocation | Use Case |
|---------|------------|----------|
| `sigma_clipped_median_mad` | Heap (Vec) | Large datasets, reusable buffer |
| `sigma_clipped_median_mad_arrayvec` | Stack (ArrayVec) | Small fixed-size data, zero heap |

Both share the same core logic via `sigma_clip_iteration()`.

### MAD to Sigma Conversion

For normally distributed data: `σ ≈ 1.4826 × MAD`

The constant is `1 / Φ⁻¹(3/4)` where Φ⁻¹ is the inverse normal CDF.

## Benchmark Results (1,024 elements)

| Function | Time |
|----------|------|
| median_f32_mut | 3.1 µs |
| sigma_clipped_median_mad (3 iterations) | 5.9 µs |

## Files

- `mod.rs` - All statistical functions
- `tests.rs` - Comprehensive tests including edge cases
- `bench.rs` - Performance benchmarks
