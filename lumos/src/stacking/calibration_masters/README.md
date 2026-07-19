# Calibration Masters

Creates, loads, saves, and applies master calibration frames (bias, dark, flat) to light frames.

`CalibrationMasters` keeps its images and derived defect map private. Build a bundle through
`from_images` or `from_files`; inspect its read-only `components()` and `defect_summary()` views.
Replacing a source master without rebuilding its defect map and prepared flat is not representable.
`save` writes the coherent bundle, including the already normalized flat divisor; `load` restores it
without repeating defect detection, flat subtraction, or per-color normalization.

`CalibrationSet<T>` is the shared four-role shape: `from_files` accepts path slices and
`from_images` accepts optional prebuilt CFA images through the same named dark, flat, bias, and
flat-dark fields.

`calibrate` validates that the light and every stored master carry the same Mono, Bayer, or X-Trans
pattern before changing the light. Missing or mismatched metadata returns `CalibrationError`;
failed validation leaves both pixels and the calibrated flag unchanged.

## Construction and cancellation

- `stack_cfa_master(paths, config, cancel)` stacks one calibration role and polls `CancelToken`
  during loading and combination.
- `CalibrationMasters::from_images(images, sigma, cancel)` builds the coherent bundle and polls
  cancellation during hot/cold defect-map scans. Cold defects are detected from the raw flat, then
  the flat is consumed into a bias/flat-dark-subtracted, per-color normalized divisor. Cancellation
  returns `Error::Cancelled`; partial masters, prepared flats, or defect maps never escape.
- `CalibrationMasters::from_files(frames, sigma)` schedules independent role stacks concurrently
  when the whole set fits the memory budget, or sequentially with the full budget per role when it
  does not. This convenience constructor currently runs those role stacks to completion.
- `CalibrationMasters::save(path)` / `load(path)` persist the prepared bundle in a versioned binary
  cache. A loaded cache is immediately ready to calibrate lights.

## Calibration Formula

The standard astrophotography calibration formula:

```
calibrated = (Light - Dark) / normalize(Flat - Bias)
```

Where:
- `Light` = raw light frame = signal + bias + thermal + vignetting
- `Dark` = master dark = bias + thermal (stored raw, not bias-subtracted)
- `Flat` = master flat = K * vignetting + bias
- `Bias` = master bias (readout offset)

### Why raw dark works

Since dark contains `bias + thermal`, subtracting it from light removes both:
```
Light - Dark = (signal + bias + thermal) - (bias + thermal) = signal * vignetting
```

No separate bias subtraction needed when a dark is available.

### Why bias must be subtracted from flat

The flat contains `K * vignetting + bias`. Normalizing without removing bias:
```
Light / (Flat / mean(Flat)) = signal * vignetting / ((K*vignetting + bias) / mean(...))
```
The bias term prevents perfect vignetting cancellation, leaving ~1-5% residual error.

With bias removed: `(Flat - Bias) / mean(Flat - Bias) = pure vignetting profile`.

## Current Limitations

- **No dark scaling**: Raw dark storage prevents scaling darks to different exposure times
  (bias component would be incorrectly scaled). Darks must match light exposure time.

## Flat Dark Support

Flat dark frames (darks taken at the flat's exposure time) can be provided to
`CalibrationMasters::from_images()` or `from_files()`. When available, the flat dark is
subtracted from the flat instead of bias during normalization:

```
normalize(Flat - FlatDark) instead of normalize(Flat - Bias)
```

This is important for narrowband imaging where longer flat exposures accumulate
significant dark current. Flat dark takes priority over bias when both are provided.

## Defect Detection

Two defect classes from two masters (a dark has no light, so dead pixels are invisible in it;
a flat reveals them as dark spots under uniform illumination):

**Hot pixels — from the master dark**, via robust per-color Median Absolute Deviation (MAD):
- MAD is robust to outliers (unlike standard deviation — the very pixels being hunted inflate σ)
- Default 5-sigma threshold (conservative, avoids false positives); clamped to ≥ 1σ
- Per-color σ with two floors, `σ = max(1.4826·MAD, median·0.1, 5e-4)`: the absolute floor
  (`5e-4`, ~33 ADU at 16-bit) stops a near-uniform stacked dark with MAD ≈ 0 from flagging its own
  warm tail; the relative floor scales it on high-dark-current sensors
- Adaptive sampling (100K px per color) above 200K pixels for fast median/MAD

**Cold/dead pixels — from the master flat**, via a local same-color-neighbor ratio: a pixel reading
below `DEAD_PIXEL_FRACTION` (0.5) of the median of its same-color local neighbors is dead. The local
reference tracks vignetting (where a global `median − kσ` cut fails) and ignores dust shadows (which
dim a pixel *and its neighbors* together, so the ratio stays ~1).

**Correction:** each defect is replaced by the median of its **same-color** CFA neighbors (mono
8-connected, Bayer stride-2, X-Trans radius-6), computed from a defect mask so clustered defects
draw only on good neighbors.

## Pipeline Order

1. Subtract master dark (removes bias + thermal)
2. Divide by the prepared normalized master flat (corrects vignetting)
3. Correct hot + cold/dead pixels (same-color neighbor median)

## Optional single-frame cosmic-ray rejection

`AlignStackConfig::cosmic_ray` enables L.A.Cosmic after calibration and before demosaic or
registration, while the data is still linear and a hit has not been spread by interpolation.
`CosmicRayConfig` controls significance, object contrast, mask growth, iterations, and empirical or
parametric noise estimation.

The implementation dispatches by sensor layout:

- Mono: the dense subsampled Laplacian detector.
- Bayer: deinterleave the four 2×2 phases, run the mono detector on each dense same-color plane,
  then reassemble the mosaic.
- X-Trans: same-color mosaic stencils selected through the 6×6 pattern.

Calibration rejects unlabelled CFA frames because neither flat normalization nor same-color
correction can be applied safely without the sensor pattern. Tight Bayer stars can become nearly
single-pixel sources in a phase plane, so per-frame rejection is best reserved for short sequences
where stack-time rejection cannot reliably outvote transients; dithered multi-frame sets should
normally rely on sigma/winsor rejection.

## References

- [Siril Calibration Documentation](https://siril.readthedocs.io/en/latest/preprocessing/calibration.html)
- [PixInsight Master Calibration Frames](https://www.pixinsight.com/tutorials/master-frames/)
- [Astropy CCD Data Reduction Guide](https://www.astropy.org/ccd-reduction-and-photometry-guide/)
