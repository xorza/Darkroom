# Multi-Threshold Deblending Algorithm

Implementation of SExtractor-style multi-threshold deblending for separating overlapping astronomical sources.

## Reference

Based on the algorithm described in:
- Bertin & Arnouts (1996), A&AS 117, 393: "SExtractor: Software for source extraction"

## Algorithm Overview

The multi-threshold deblending algorithm separates overlapping sources by:

1. **Building an areal profile tree** - The intensity distribution of a connected component is represented as a tree structure where branches form when the image is thresholded at successively higher levels
2. **Applying a contrast criterion** - Branches are considered distinct objects only if their flux exceeds a minimum fraction of the parent branch's flux
3. **Assigning pixels** - Pixels below the separation threshold are assigned to their nearest peak

## Threshold Computation

Thresholds are computed with **exponential spacing** between detection level and peak value:

```
threshold[i] = low * (high/low)^(i/n)
```

Where:
- `low` = detection threshold (minimum pixel value in component)
- `high` = peak pixel value
- `n` = number of threshold levels (DEBLEND_NTHRESH)
- `i` = threshold index (0 to n)

Exponential spacing provides finer resolution near the detection threshold where blended objects typically separate first.

## Tree Construction

At each threshold level:
1. Find all pixels above the threshold
2. Identify connected regions (using 8-connectivity)
3. When a parent region splits into multiple child regions, create child nodes in the tree
4. Track flux (sum of pixel values) for each node

## Contrast Criterion

A branch is considered a separate object if:

```
child_flux >= min_contrast * parent_flux
```

Key points:
- Comparison is against **parent flux**, not root flux (per SExtractor algorithm)
- `min_contrast` = DEBLEND_MINCONT parameter (default: 0.005 = 0.5%)
- Setting `min_contrast = 1.0` effectively disables deblending
- Lower values deblend more aggressively but may create spurious detections

## Pixel Assignment

After identifying significant branches:
1. Collect leaf nodes that pass the contrast criterion
2. Assign each pixel to the nearest peak (Euclidean distance)
3. SExtractor uses probability-based assignment with bivariate Gaussian fits; this implementation uses simpler nearest-peak assignment

## Configuration Parameters

| Parameter | SExtractor Name | Default | Description |
|-----------|-----------------|---------|-------------|
| `n_thresholds` | DEBLEND_NTHRESH | 32 | Number of threshold levels |
| `min_contrast` | DEBLEND_MINCONT | 0.005 | Minimum flux ratio for separate object |
| `min_separation` | - | 3 | Minimum peak separation (pixels) |

## Typical Values

From astronomical surveys:
- **HUDF/HRC**: DEBLEND_NTHRESH=32, DEBLEND_MINCONT=0.100
- **Dark Energy Survey**: DEBLEND_NTHRESH=32, DEBLEND_MINCONT=0.001
- **General use**: DEBLEND_NTHRESH=32, DEBLEND_MINCONT=0.005

## Implementation Notes

### Differences from SExtractor

1. **Pixel assignment**: Uses nearest-peak instead of Gaussian probability
2. **Peak merging**: Peaks closer than `min_separation` are merged (SExtractor doesn't have this)
3. **SmallVec optimization**: Returns `SmallVec<[DeblendedCandidate; MAX_PEAKS]>` for stack allocation

### Performance Considerations

- Tree building iterates over component pixels `n_thresholds + 1` times
- Connected component finding uses HashMap for O(1) pixel lookup
- For very large components, consider reducing `n_thresholds`

### Edge Cases

- **Single pixel**: Returns single object without tree building
- **Flat profile**: No branching occurs, returns single object
- **Peak barely above threshold**: Returns single object (< 1% margin check)

## File Structure

- `multi_threshold.rs` - Main algorithm implementation
- `local_maxima.rs` - Faster alternative for well-separated stars
- `mod.rs` - Module exports and shared types (`ComponentData`, `Pixel`, `DeblendedCandidate`)

## Testing

22 tests cover:
- Single/multiple star deblending
- Contrast criterion at boundary
- Pixel conservation
- Various star configurations (horizontal, vertical, diagonal)
- Edge cases (empty, single pixel, flat profile)

Run with: `cargo nextest run -p lumos multi_threshold`

## References

- [SExtractor Documentation](https://sextractor.readthedocs.io/)
- [Bertin & Arnouts 1996 (ADS)](https://ui.adsabs.harvard.edu/abs/1996A%26AS..117..393B)
- [sep (Python implementation)](https://sep.readthedocs.io/)
