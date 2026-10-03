# Issues

- `lumos/src/background_mesh/tile_stats/mod.rs` `TileStats::compute` — a tile's σ is the MAD spread of its raw values, so the sky's own variation across the tile counts as noise: on `background/tests/synthetic_skies.rs`'s gradient sky (0.05 → 0.25 over 256 px, tile 64) the noise map reads 0.019 against the camera's 0.0017, and the vignette and nebula skies read 0.022 and 0.038. Through the Pearson-mode branch (`|mean − median| < 0.3σ`) the inflated σ also lets one bright star pull its tile's sky down by up to 5e-3.
