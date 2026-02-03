# Cosmic Ray Detection Module

L.A.Cosmic-based cosmic ray identification, based on van Dokkum 2001, PASP 113, 1420.

## Core Principle

Cosmic rays have sharper edges than astronomical sources (which are smoothed by the PSF/atmosphere). The Laplacian (second derivative) responds strongly to sharp edges, enabling discrimination between CRs and real stars.

## Usage in the Pipeline

The module is used in **two ways**:

1. **Per-star metric** (`compute_laplacian_snr()`): The primary CR rejection method. Computes Laplacian SNR at each star candidate's position during centroiding. Stars with high values (>50) are flagged as cosmic rays in quality filtering. This avoids the cost of full-frame processing.

2. **Full-frame detection** (`detect_cosmic_rays()`): Standalone function for image-level CR detection and masking. Currently unused by the main detection pipeline (`#[allow(dead_code)]`).

## Implementation vs Original L.A.Cosmic Paper

The implementation is simplified compared to the original van Dokkum 2001 algorithm. These simplifications are acceptable for the star detection use case where CRs are primarily rejected as false detections rather than cleaned from the image.

### What's Implemented

- Standard 3x3 discrete Laplacian kernel `[0,1,0; 1,-4,1; 0,1,0]`
- SIMD-accelerated Laplacian computation (SSE4.1/AVX2, NEON)
- Fine structure detection via `original - median3` (captures sharp features)
- Laplacian-to-flux ratio criterion for CR identification
- Mask growing to catch CR wings
- Per-star Laplacian SNR metric for quality filtering

### Differences from the Original Paper

| Feature | Original L.A.Cosmic (van Dokkum 2001) | This Implementation |
|---------|---------------------------------------|---------------------|
| **Subsampling** | 2x block-replicate before Laplacian, then reduce back. Improves sensitivity to subpixel CRs. | Native resolution only. Sufficient for star detection where CRs are rejected as false positives. |
| **Fine structure** | `median3(image) - median7(image)` (difference of two median scales isolates point-source structure) | `original - median3` (simpler, still captures sharp features but less selective) |
| **Detection criterion** | `snr > sigclip AND snr / fine_structure > objlim` (objlim typically 2-5) | `lapl_snr > sigma_clip AND obj_above_bg > obj_lim AND lapl_to_flux > 3.0` (laplacian-to-flux ratio as alternative discriminant) |
| **Noise model** | `sqrt(gain * median5(image) + readnoise^2) / gain` (in electrons) | Uses background noise map directly |
| **Iterative cleaning** | 4 iterations: detect CRs, replace with local median, repeat | Single-pass detection |
| **Mask growing** | Two-stage: dilate + lower threshold on neighbors (`sigfrac * sigclip`) | Single-stage dilation with elevated-pixel criterion |

### Why These Simplifications Are Acceptable

The main pipeline does **not** use full-frame CR detection. Instead, it computes `compute_laplacian_snr()` per star candidate during centroiding. This per-star metric effectively discriminates CRs from stars:

- **Cosmic rays**: Single-pixel spikes produce very high Laplacian magnitude at the peak, yielding Laplacian SNR >> 50.
- **Real stars**: PSF-smoothed profiles produce moderate Laplacian magnitude, yielding Laplacian SNR << 50 (typically 5-20 for well-sampled stars).

The per-star approach avoids the computational cost of full-frame processing while still effectively rejecting CR false positives.

### If Full L.A.Cosmic Is Needed

To bring `detect_cosmic_rays()` closer to the original paper:

1. Add 2x block replication before Laplacian and reduction after
2. Change fine structure to `median3 - median7` instead of `original - median3`
3. Use `snr / fine_structure > objlim` as the primary discrimination criterion
4. Add iterative cleaning (detect, replace with median, repeat 4x)
5. Add two-stage mask growing with `sigfrac` neighbor threshold
6. Use gain-corrected noise model in electrons

## Key Functions

- `compute_laplacian()` - 3x3 discrete Laplacian with SIMD acceleration
- `compute_laplacian_snr()` - Per-star Laplacian SNR metric (used by centroid module)
- `compute_fine_structure()` - Small-scale structure detection (original - median3)
- `detect_cosmic_rays()` - Full-frame L.A.Cosmic detection (currently unused)

## References

- van Dokkum, P.G., 2001, PASP, 113, 1420: "Cosmic-Ray Rejection by Laplacian Edge Detection"
- lacosmic Python package: https://lacosmic.readthedocs.io/
