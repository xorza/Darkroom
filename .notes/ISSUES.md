# Open issues

## imaginarium: the README names a quickbench dev-dependency that does not exist

`imaginarium/README.md` says the benchmark harness lives in `../quickbench/` as a
`dev-dependency`. `imaginarium/Cargo.toml` has no such dependency, and darkroom no longer has a
`quickbench` submodule.

## lumos: a mosaic's noise and sky skip the colours of the rows the subsampling steps over

`DifferenceNoise::for_each_pair` (each colour's noise) and `MosaicNoise::for_each_sample` (each
colour's sky) visit every `row_step`-th row, with `row_step = pixel_count / 3 000 000`. When
`row_step` shares a factor with the pattern's period, whole rows of the pattern are never visited.
On a Bayer mosaic an even `row_step` visits only even rows, so the colour at odd rows and odd
columns, blue under RGGB and red under BGGR, measures noise 0 and sky 0. That holds for 6 to 9 MP,
12 to 15 MP, 18 to 21 MP and 24 to 27 MP frames: at 6000 × 4000 `row_step` is 8, at 5800 × 2900,
4096 × 4096 and 4097 × 4096 it is 5. On an X-Trans mosaic, whose period is 6, an even `row_step`
measures only half of the pattern's rows.

Both `FrameStats::measure` of a mosaic and the noise `CfaImage::demosaic` stores in the demosaiced
frame's metadata read these values. With the light preset's noise weighting, `drizzle_cfa_stack`
of twelve 6000 × 4000 RGGB mosaics fails with `DrizzleError::Stack(NoNoiseToWeigh { index: 0 })`.
`calibrate_align_stack` of 16-bit files of that size does not fail, because the quantization
floor keeps the variance above 0, but its blue noise and blue sky are not measured.
