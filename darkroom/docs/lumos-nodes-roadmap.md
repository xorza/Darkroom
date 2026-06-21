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

## Editor foundations — done

The reusable editor work the astro nodes lean on is already in place:

- **Enum config editing** — `value_editor.rs` renders enum ports as a dropdown
  (`ComboBox`) over the declared variants.
- **Connection type validation** — incompatible ports won't snap
  (`connection_ui.rs`, `types_compatible`); `Null` is a wildcard for boundary
  "+" placeholders.
- **Per-type port colors + type tooltips** — ports and wires read by `DataType`
  (`gui/node/port_color.rs`); hovering a port circle or label shows the type.

One prerequisite remains, and it's just authoring: **custom data types** for the
astro payloads (`AstroFrame`, `Masters`) — built in Phase 1 below.

One non-blocking concern: **lumos work is heavy synchronous CPU**
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
- Nodes show live thumbnails via `gen_preview` (autostretched for visibility).

## Roadmap

> Phase 0 (darkroom editor foundations — enum dropdown editor, connection type
> validation, per-type port colors, port type tooltips) is **done**; see *Editor
> foundations* above.

### Phase 1 — Astro support inside `lens` (mirrors the image-node pattern)

1. Add `lumos` as a dependency in `lens/Cargo.toml` (alongside `imaginarium`).
2. **`AstroFrame`** custom type (`lens/src/astro_frame.rs`): wrap
   `lumos::AstroImage`, impl `CustomValue` (+ `ASTRO_DATA_TYPE` static),
   `gen_preview` = autostretch → `imaginarium::Image` thumbnail. Pattern =
   `lens/src/image.rs`.
3. **`Masters`** custom type (`lens/src/masters.rs`) wrapping
   `lumos::CalibrationMasters`.
4. New `AstroFuncLib::default()` in `lens/src/astro_funclib.rs` (sibling to
   `ImageFuncLib`), exported from `lens/src/lib.rs`. Add one trivial node
   (**Load Astro Image**: `FsPath` file → `AstroFrame`) to prove it end-to-end,
   then merge it at `darkroom/src/core/func_lib.rs:27`:
   `func_lib.merge(AstroFuncLib::default());`

### Phase 2 — Headline nodes

5. **Build Masters node.** Inputs: `darks/flats/bias/flat_darks` (`FsPath`
   Directory, optional), `sigma` (Float = 5.0). Output: `Masters`. Lambda globs
   each dir, calls `CalibrationMasters::from_files`, inside `spawn_blocking`.
6. **Stack Lights node.** Inputs: `lights` (Directory), `masters` (`Masters`,
   optional), `detection`/`registration`/`combine` presets (Enum), `σ-low`/`σ-high`
   (Float), `reference` (Enum). Outputs: `image` + `coverage` + `weight`
   (`AstroFrame`). Lambda builds `AlignStackConfig` from preset + overrides →
   `calibrate_align_stack`, in `spawn_blocking`; route lumos progress to
   `ctx.info`.

### Phase 3 — Processing nodes (fast fan-out)

7. One node each, `AstroFrame → AstroFrame`, wrapping the in-place ops:
   **Auto Stretch, Background Extract, Denoise, SCNR, HDR Compress, Local
   Contrast, Neutralize Background**, plus **Save Astro Image** and **Star Detect**
   (→ count/overlay). Each: clone input frame, mutate, output. Mostly boilerplate
   now that the type + enum editor exist.

### Phase 4 — Polish / advanced

8. Split detection/registration/warp/combine into composable nodes (`StarSet`,
   `WarpTransform` custom types) for power users; add **Drizzle**.
9. Optional **config-builder nodes** (e.g. a `Stack Config` node outputting a
   `StackConfig` custom value) if presets+overrides prove too limiting — defer
   until needed.
10. AstroFrame ⇄ lens `Image` bridge nodes so astro output can flow into the
    existing imaginarium nodes (trivial now that both live in `lens`).

## Brief implementation sketches

**Custom type (Phase 1):**

```rust
// lens/src/astro_frame.rs
pub static ASTRO_TYPE_DEF: LazyLock<Arc<TypeDef>> = LazyLock::new(|| Arc::new(TypeDef {
    type_id: "….".into(), display_name: "AstroFrame".into(),
}));
pub static ASTRO_DATA_TYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::Custom(ASTRO_TYPE_DEF.clone()));

#[derive(Debug)]
pub struct AstroFrame { pub image: lumos::AstroImage, preview: Slot<imaginarium::Image> }
impl CustomValue for AstroFrame { /* type_def, as_any, gen_preview = autostretch→thumb */ }
```

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
- **Home:** all astro types + nodes live in `lens` (new modules: `astro_frame.rs`,
  `masters.rs`, `astro_funclib.rs`); `lens` gains a `lumos` dependency.

With the editor foundations done, **Phase 1** (custom astro types in `lens`) is
the next slice.
