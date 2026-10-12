# Open issues

## lumos: a 6000 × 4000 Bayer mosaic measures no noise for its odd-row, odd-column colour

`FrameStats::measure` and `FrameStats::measure_with_spread` of a 6000 × 4000 Bayer mosaic give
noise 0 and sky 0 for the colour at odd rows and odd columns: blue under RGGB, red under BGGR. The
other two colours measure as expected. The same scene at 5800 × 2900, 4096 × 4096 and
4097 × 4096 measures all three colours, the odd one at about 0.0011. With the light preset's noise
weighting, `drizzle_cfa_stack` of twelve such mosaics fails with
`DrizzleError::Stack(NoNoiseToWeigh { index: 0 })`. Seen on `Field::Typical.mosaic_exposure` of
`lumos::bench::scene`.
