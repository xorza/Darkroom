# Lumos nodes in Darkroom — roadmap

Goal: expose `lumos` astronomical processing (calibration masters, stacking,
post-processing) as nodes in the Darkroom visual editor, by **extending the
`lens` crate** (no new crate). Covers the editor work needed for configs, custom
data types, and connection-type validation.

## What already exists (the substrate)

**scenarium** gives us everything to define typed nodes:

- A node = a `Func` (`scenarium/src/function.rs:56`) with `inputs: Vec<FuncInput>`,
  `outputs: Vec<FuncOutput>`, an async `lambda`, a `category`, and
  `required_contexts`.
- Port types are `DataType` (`scenarium/src/data.rs:100`):
  `Null/Float/Int/Bool/String/FsPath/Custom(TypeDef)/Enum(EnumDef)`. `FsPath`
  already supports a **Directory** picker mode.
- Runtime values are `DynamicValue` (`Unbound/Static/Custom`); arbitrary payloads
  ride on the `CustomValue` trait (`data.rs:36`).
- Registration is just `func_lib.add(Func{…})`, collected into a `FuncLib`.

**lens** is both the template *and* the home for this work (`lens/src/`):

- `Image` (`lens/src/image.rs`) wraps `imaginarium::ImageBuffer` as a `CustomValue`
  with an `IMAGE_DATA_TYPE` static and an async thumbnail `gen_preview`.
- Enums map via `DataType::from_enum::<E>(uuid, name)`
  (`lens/src/conversion_format.rs`).
- A shared `VisionCtx` (GPU `ProcessingContext`) is pulled via `required_contexts`
  (`lens/src/vision_ctx.rs`).
- `ImageFuncLib::default()` adds all nodes; merged into the app at
  `darkroom/src/core/func_lib.rs:23` (`runtime_func_lib()`).

**darkroom** auto-discovers nodes — **anything in the merged `FuncLib` appears in
the right-click palette by category with zero UI code**
(`gui/canvas/new_node_ui.rs`). Scalar/string consts and `FsPath` (incl. the
folder picker) already get inline editors (`gui/node/value_editor.rs`).

**lumos** exposes a clean, wrappable API:
`CalibrationMasters::from_files(CalibrationFrames, sigma)`,
`calibrate_align_stack(light_paths, &masters, &AlignStackConfig) -> AlignStackResult`,
plus in-place `AstroImage` ops (`stretch`, `denoise`, `extract_background`, `scnr`,
`enhance_local_contrast`, `compress_dynamic_range`, `neutralize_background`) and
rich config presets (`StackConfig::sigma_clipped`, `StarDetectionConfig::wide_field`,
`RegistrationConfig::fast`, …).

## Done so far

**Editor foundations** (Phase 0) — reusable across all node types:

- **Enum config editing** — `value_editor.rs` renders enum ports as a dropdown
  (`ComboBox`) over the declared variants.
- **Connection type validation** — incompatible ports won't snap
  (`connection_ui.rs`, `types_compatible`); `Null` is a wildcard for boundary
  "+" placeholders.
- **Per-type port colors + type tooltips** — ports and wires read by `DataType`
  (`gui/node/port_color.rs`); hovering a port circle or label shows the type.

**Astro types + proof node** (Phase 1) — in the `lens` crate (now depends on
`lumos`):

- **`AstroFrame`** (`lens/src/astro_frame.rs`) wraps `lumos::AstroImage` as a
  `CustomValue` with a CPU RGBA_U8 thumbnail — planar channels sampled straight to
  RGBA in one pass, no display stretch (so a *linear* FITS/RAW frame previews
  dark); `gen_preview` runs synchronously and parks the result in a `Slot`.
- **`Masters`** (`lens/src/masters.rs`) wraps `lumos::CalibrationMasters`.
- **`AstroFuncLib`** (`lens/src/astro_funclib.rs`) with `load_astro_image`
  (`FsPath` → `AstroFrame`, decode off-thread via `spawn_blocking`); merged at
  `darkroom/src/core/func_lib.rs`. A new `AstroFrame` branch in `node_values.rs`
  uploads its preview.

**Build Masters node** (Phase 2, partial) — `build_masters` in `AstroFuncLib`:
optional `darks`/`flats`/`bias`/`flat_darks` directory inputs (new shared
`ASTRO_DIR_DATA_TYPE`) + `sigma` (Float = 5.0) → `Masters`. Globs each folder via
`common::file_utils::astro_image_files` and calls `CalibrationMasters::from_files`
inside `spawn_blocking`.

One non-blocking concern carried into the node phases: **lumos work is heavy
synchronous CPU**
(rayon/nalgebra). Node lambdas run on the tokio worker, so each must offload via
`tokio::task::spawn_blocking` to avoid stalling the scheduler.

## How it looks in darkroom

A user right-clicks → picks from a new **`astro`** category:

```
[Build Masters]                    [Stack Lights]                  [Auto Stretch] → [Background Extract] → [Denoise] → [SCNR] → out
 darks   dir ──┐                    lights  dir ───────┐
 flats   dir ──┤                    masters ●──────────┤ (CalibrationMasters wire)
 bias    dir ──┤                    preset  [wide ▼]   ├──● image (AstroFrame) ──▶ ...
 sigma   5.0   ├──● masters ●───────●                  │
               │                    combine [sigma ▼]  ├──● coverage (AstroFrame)
                                    σ-low   2.0         │
                                    σ-high  3.0         └──● weight   (AstroFrame)
```

- Wires carry typed custom values (`AstroFrame`, `CalibrationMasters`), colored by
  type; incompatible drops won't snap.
- Directory inputs use the existing folder picker; the node globs the dir for
  RAW/FITS.
- Config = a **preset enum dropdown + a few key override ports** (recommended over
  exploding all ~40 config fields into ports).
- Nodes show live thumbnails via `gen_preview` (linear — dark without a stretch).

## Roadmap

> **Done:** Phase 0 (editor foundations) and Phase 1 (astro custom types +
> `load_astro_image`). See *Done so far* above.

### Phase 2 — Headline nodes

1. **Build Masters node.** Inputs: `darks/flats/bias/flat_darks` (`FsPath`
   Directory, optional), `sigma` (Float = 5.0). Output: `Masters`. Lambda globs
   each dir, calls `CalibrationMasters::from_files`, inside `spawn_blocking`.
2. **Stack Lights node.** Inputs: `lights` (Directory), `masters` (`Masters`,
   optional), `detection`/`registration`/`combine` presets (Enum), `σ-low`/`σ-high`
   (Float), `reference` (Enum). Outputs: `image` + `coverage` + `weight`
   (`AstroFrame`). Lambda builds `AlignStackConfig` from preset + overrides →
   `calibrate_align_stack`, in `spawn_blocking`; route lumos progress to
   `ctx.info`.

### Phase 3 — Processing nodes (fast fan-out)

3. One node each, `AstroFrame → AstroFrame`, wrapping the in-place ops:
   **Auto Stretch, Background Extract, Denoise, SCNR, HDR Compress, Local
   Contrast, Neutralize Background**, plus **Save Astro Image** and **Star Detect**
   (→ count/overlay). Each: clone input frame, mutate, output. Mostly boilerplate
   now that the type + enum editor exist.

### Phase 4 — Polish / advanced

4. Split detection/registration/warp/combine into composable nodes (`StarSet`,
   `WarpTransform` custom types) for power users; add **Drizzle**.
5. Optional **config-builder nodes** (e.g. a `Stack Config` node outputting a
   `StackConfig` custom value) if presets+overrides prove too limiting — defer
   until needed.
6. AstroFrame ⇄ lens `Image` bridge nodes so astro output can flow into the
   existing imaginarium nodes (trivial now that both live in `lens`).

## Brief implementation sketches

**Node lambda offloading heavy work (Phase 2):**

```rust
lambda: async_lambda!(move |ctx, _, _, inputs, _, outputs| {
    let lights_dir = inputs[0].value.as_string().unwrap().to_owned();
    let masters = inputs[1].value.as_custom::<Masters>().cloned();   // optional
    let cfg = build_align_stack_config(&inputs);                     // preset + overrides
    let res = tokio::task::spawn_blocking(move || {
        let paths = glob_raw(&lights_dir);
        lumos::calibrate_align_stack(&paths, &masters.unwrap_or_default().0, &cfg)
    }).await.unwrap().map_err(anyhow::Error::from)?;
    outputs[0] = DynamicValue::from_custom(AstroFrame::new(res.image));
    Ok(())
})
```

## Decision points (recommendations baked in above)

- **Config surfacing:** preset enum + few override ports *(recommended)* vs. full
  per-field ports vs. config-builder nodes. Start with presets; it needs only the
  enum editor.
- **Stacking granularity:** one mega `Stack Lights` node *(recommended for v1)* vs.
  composable detect/register/warp/combine nodes (Phase 4).
- **Home:** astro types + nodes live in `lens` (`astro_frame.rs`, `masters.rs`,
  `astro_funclib.rs`); `lens` depends on `lumos`. *(Decided + done in Phase 1.)*

**Phase 2** (Build Masters + Stack Lights) is the next slice.
