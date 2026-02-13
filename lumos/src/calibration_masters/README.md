# Calibration Masters

Creates, loads, saves, and applies master calibration frames (bias, dark, flat) to light frames.

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
`CalibrationMasters::new()` or `from_raw_files()`. When available, the flat dark is
subtracted from the flat instead of bias during normalization:

```
normalize(Flat - FlatDark) instead of normalize(Flat - Bias)
```

This is important for narrowband imaging where longer flat exposures accumulate
significant dark current. Flat dark takes priority over bias when both are provided.

## Hot Pixel Detection

Uses Median Absolute Deviation (MAD) on the raw master dark:
- MAD is robust to outliers (unlike standard deviation)
- 5-sigma threshold (conservative, avoids false positives)
- Sigma floor of `median * 0.1` prevents stacked darks' tiny MAD from over-detecting
- Per-channel analysis for RGB data
- 8-neighbor median replacement

## Pipeline Order

1. Subtract master dark (removes bias + thermal)
2. Divide by normalized master flat (corrects vignetting)
3. Correct hot pixels (median of neighbors)

## References

- [Siril Calibration Documentation](https://siril.readthedocs.io/en/latest/preprocessing/calibration.html)
- [PixInsight Master Calibration Frames](https://www.pixinsight.com/tutorials/master-frames/)
- [Astropy CCD Data Reduction Guide](https://www.astropy.org/ccd-reduction-and-photometry-guide/)
