# Local Maxima Deblending Algorithm

Fast deblending algorithm using local maxima detection and Voronoi partitioning.

## References

- Stetson (1987), PASP 99, 191: "DAOPHOT - A computer program for crowded-field stellar photometry"
- [Photutils find_peaks](https://photutils.readthedocs.io/en/latest/user_guide/detection.html)
- [DAOStarFinder](https://photutils.readthedocs.io/en/stable/api/photutils.detection.DAOStarFinder.html)

## Algorithm Overview

The local maxima deblending algorithm separates overlapping sources by:

1. **Finding local maxima** - Pixels brighter than all 8 neighbors
2. **Filtering by prominence** - Peaks must exceed a fraction of the primary peak
3. **Filtering by separation** - Peaks must be sufficiently far apart
4. **Voronoi partitioning** - Assign pixels to nearest peak

This is faster than multi-threshold deblending but less accurate for heavily blended sources.

## Local Maximum Detection

A pixel is considered a local maximum if its value is strictly greater than all 8 neighbors:

```
[NW] [N ] [NE]
[W ] [C ] [E ]    C > all neighbors
[SW] [S ] [SE]
```

Edge and corner pixels are handled by only comparing to existing neighbors.

### Comparison with DAOFIND

DAOFIND (Stetson 1987) uses a more sophisticated approach:
1. Convolves image with Gaussian kernel matching expected PSF
2. Finds local maxima in the convolved image
3. Applies sharpness and roundness filters

Our implementation skips convolution (assumes pre-filtered image) and shape filters (applied post-detection).

## Prominence Criterion

Peaks are filtered by prominence relative to the global maximum:

```
peak_value >= min_prominence * global_max_value
```

Where:
- `min_prominence` = fraction threshold (default: 0.3 = 30%)
- `global_max_value` = brightest pixel in the component

This prevents noise spikes from creating false deblending.

## Separation Criterion

Peaks must be separated by a minimum Euclidean distance:

```
sqrt((x1-x2)² + (y1-y2)²) >= min_separation
```

When peaks are too close:
- Only the brighter peak is kept
- Dimmer peaks within `min_separation` are discarded

This is implemented efficiently using squared distances to avoid sqrt.

## Voronoi Partitioning

After identifying peaks, pixels are assigned to the nearest peak:

```
nearest_peak = argmin_i( dist(pixel, peak_i) )
```

Properties:
- Each pixel belongs to exactly one deblended object
- Total area is conserved (sum of deblended areas = original area)
- Creates convex regions around each peak (Voronoi cells)

### Voronoi in Astronomy

Voronoi tessellation is widely used in astronomy for:
- Image segmentation and source deblending
- Adaptive binning for spectroscopic data
- Large-scale structure analysis

## Configuration Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `min_separation` | 3 | Minimum peak separation (pixels) |
| `min_prominence` | 0.3 | Minimum peak brightness as fraction of primary |

## Performance Characteristics

- **Time complexity**: O(N × P) where N = pixels, P = peaks (typically P << N)
- **Space complexity**: O(1) extra - uses `ArrayVec` for stack allocation
- **Peak limit**: MAX_PEAKS = 8 (avoids heap allocation)

## Comparison with Multi-Threshold

| Aspect | Local Maxima | Multi-Threshold |
|--------|--------------|-----------------|
| Speed | Fast | Slower |
| Accuracy | Good for separated sources | Better for blended sources |
| Memory | Stack-allocated | Heap-allocated tree |
| Criterion | Prominence | Contrast (flux ratio) |

Use local maxima for:
- Well-separated stars
- Real-time processing
- Initial quick detection

Use multi-threshold for:
- Crowded fields
- Heavily blended sources
- Accurate flux partitioning

## Implementation Notes

### ArrayVec Optimization

Uses `ArrayVec<Pixel, MAX_PEAKS>` instead of `Vec<Pixel>`:
- Avoids heap allocation for common case (≤8 peaks)
- Fixed capacity prevents unbounded memory growth
- Excess peaks are simply not added

### Peak Replacement

When a new peak is too close to an existing one:
```rust
if new_peak.value > existing_peak.value {
    existing_peak = new_peak;  // Replace with brighter
}
```

### Edge Handling

Local maximum check at image boundaries:
- Corner pixels: compare 3 neighbors
- Edge pixels: compare 5 neighbors
- Interior pixels: compare 8 neighbors

## File Structure

- `local_maxima.rs` - This algorithm
- `multi_threshold.rs` - SExtractor-style tree-based deblending
- `mod.rs` - Shared types and exports

## Testing

16 tests cover:
- Single/multiple peak detection
- Prominence and separation filtering
- Area conservation
- Edge cases (empty, boundaries)
- Peak replacement logic

Run with: `cargo nextest run -p lumos local_maxima`

## References

- [DAOFIND Algorithm (Photutils)](https://photutils.readthedocs.io/en/stable/api/photutils.detection.DAOStarFinder.html)
- [find_peaks Documentation](https://photutils.readthedocs.io/en/latest/user_guide/detection.html)
- [Voronoi Tessellation in Astronomy](https://arxiv.org/abs/2012.08965)
- [Source Detection Review (MNRAS)](https://academic.oup.com/mnras/article/422/2/1674/1040345)
