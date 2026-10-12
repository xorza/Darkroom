# lumos benches: finish the inventory

Scope: the work the bench inventory left, and the open issues in `.notes/ISSUES.md`. The bench
rules are the ones `lumos/src/bench/mod.rs` states. Decision already taken: `libc` may become an
optional Linux dependency for the cold reads (item 2).

## Items

Done in this order; each is deleted when it is committed.

2. **The page eviction compiles for the benches.** The eviction is `#[cfg(test)]` in
   `lumos/src/platform/mod.rs` (`MappedFile::evict`), `platform/linux/mod.rs`, `platform/macos.rs`,
   `platform/windows.rs` and `frame_store/stored_plane.rs` (`StoredPlane::evict`).
   - `lumos/Cargo.toml`: `[target.'cfg(target_os = "linux")'.dependencies] libc = { workspace =
     true, optional = true }`, and `bench = ["internals", "dep:criterion", "dep:libc"]`. The
     Linux dev-dependency stays for the tests.
   - Gate the eviction `any(test, feature = "bench")` in those files, with the Linux `page_size`
     it uses. `resident_share` stays `#[cfg(test)]`: split the gated mods so a bench build has no
     dead code.
   - A capability on the platform entity, `MappedFile::EVICTS: bool`, true on Linux and false
     elsewhere, so a bench registers a cold variant only where it is true. No `cfg(target_os)`
     outside `platform/`.
   - `StoredImage::evict` (the branch removed it) and `StoredFrame::evict`: every plane a read
     touches, channels, quality planes, samples and flags, through `StoredPlane::evict`.
   - Test: `StoredFrame::evict` of a spilled frame with quality planes leaves every plane's
     `resident_share` at 0 where `EVICTS` is true, and the frame reads back the same; extend
     `an_evicted_plane_reads_from_the_disk` or put it beside it.

3. **`kernel/frame_store/spill/frame/read/cold/typical`** in `lumos/src/frame_store/bench.rs`:
   the `read/warm` bench with every page of the frame evicted before each pass, the eviction in the
   untimed setup of `iter_batched` with `BatchSize::PerIteration`. Registered only where
   `MappedFile::EVICTS` holds. Update the module doc.

4. **`stage/park/warp_from_map/cold/typical`** in `lumos/src/pipeline/bench.rs`: the `warm` bench
   with the parked image evicted before each warp, as in item 3. Update `park`'s doc.

5. **`stage/combine/light_preset/spilled_cold/30xtypical`** in `lumos/src/combine/bench.rs`: the
   `spilled` bench with every spilled frame evicted after the setup writes it, so the combine reads
   the disk. Registered only where `MappedFile::EVICTS` holds. Update the module doc.

6. **`pipeline/drizzle/rggb/12xtypical`**.
   - `bench::scene`: `Field::mosaic_exposure(size, cfa_type, index)`, exposure `index` of the
     sequence calibrated and dithered, with its cosmic-ray hits:
     `self.hit(size, cfa_type, Readout::Calibrated, Shot::Sequence { index, aligned: false })`.
     Extend `a_field_keeps_its_density_on_a_smaller_frame` with its dither and white balance.
   - `bench::files`: `Files::calibrated_mosaics(field, cfa_type)`, [`LIGHTS`] such exposures saved
     by `CfaImage::save_fits`. Extend `each_file_loads_back_as_written` with one.
   - The bench, beside `pipeline::bench::pipeline` or in `lumos/src/drizzle/bench.rs` as a
     `pipeline` entry that `crate::bench::pipeline` calls: `drizzle_cfa_stack` over the files,
     each `DrizzleFrame` with `WarpTransform::new(scene::dither(FRAME, index))`, under
     `DrizzleConfig::default()` with its `ingest` cache in the files' directory, and
     `StackConfig::light()`. Throughput `LIGHTS × FRAME` pixels.
   - Run it once with `-- --test` to show the light preset's noise weighting now succeeds.

7. **The sampling of seconds-long iterations.** `stage/combine` (1.96 s an iteration) and
   `stage/drizzle` (6.91 s) are sampled at `Scale::Frame`, whose 3 s window holds neither, so
   criterion warns "Unable to complete 10 samples" on every run; the `pipeline` groups take longer
   still at `Scale::Pipeline`'s 10 s. Measured on the Linux laptop, x86-64-v3.
   - Rename `Scale::Pipeline` to `Scale::Set`, "a set of frames, in memory or in files: seconds an
     iteration", and sample every set group with it: `stage/combine`, `stage/drizzle` and the
     `pipeline` tier.
   - Find the established way criterion benches of seconds an iteration are sampled without the
     warning, and apply it. If it is a measurement time per group, derive it from each group's
     measured iteration and say on the group where the figure comes from.
   - Update `Scale`'s docs and `lumos/src/bench/mod.rs` where they name the scales.

8. **The `imaginarium` README names a quickbench dev-dependency that does not exist.**
   `imaginarium/README.md:15` says the benchmark harness lives in `../quickbench/` as a
   dev-dependency; `imaginarium/Cargo.toml` has none, and darkroom has no `quickbench` submodule
   any more. Remove the line. `imaginarium` is a submodule, a repository of its own: commit the
   edit there, and leave its push and the gitlink bump in darkroom to the user. Delete the issue
   from `.notes/ISSUES.md`.
