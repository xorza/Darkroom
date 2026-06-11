# ML filters (ONNX backend)

Optional ML-based filters behind the **`ml`** cargo feature, run through **`tract`** — Sonos's
pure-Rust ONNX inference engine — on CPU. No GPU, no C++/ONNX-Runtime dependency. **lumos bundles no
model weights**; each filter loads a `.onnx` the *caller supplies*.

Currently: **`star_removal`** (StarNet-style).

## Why caller-supplied weights (the licensing reality)

The strong astro CNNs are not redistributable:

- **StarXTerminator / NoiseXTerminator / BlurXTerminator** — commercial; EULA forbids use outside the
  paid plugin. Off the table.
- **StarNet2 (v2.5.2)** — the `LICENSE.txt` is **non-transferable**, "solely for astrophotography
  image processing", and forbids using the software/outputs "to create any commercial software,
  including for training neural networks or other image processing systems." So **lumos cannot ship
  or redistribute `StarNet2_weights.onnx`.**

→ The backend is **generic**: lumos provides the tract runner + tiling; the user points it at their
own legally-obtained ONNX (`StarRemovalConfig::weights` / the `STARNET2_ONNX` env in the test). That
keeps lumos clean of the license.

## StarNet2 model contract (verified by inspecting `StarNet2_weights.onnx`)

The shipped StarNet2 model (`tf2onnx 1.17.0`, opset 17, ~131 MB):

| | |
|---|---|
| **Input** `x:0` | `float32`, **`[batch, 512, 512, 3]`** — **NHWC**, RGB, spatial fixed at 512 |
| **Output** `Identity:0` | `float32`, `[batch, 512, 512, 3]` — the **starless image directly** |
| **Tiling** | 512×512 window, **stride 256** (50% overlap); image ≥ 512² |
| **Range** | expects **stretched display data in `[0,1]`** — *no internal stretch* (no `Exp`/`Log`/`Pow` ops). Run it **after** the stretch. |
| **Stars** | derived by **unscreen**: `stars = 1 − (1−orig)/(1−starless)` (screen-blend inverse), not plain subtraction |

**Ops** (all standard, all tract-supported): `Conv ×17, BatchNormalization ×14, Relu ×9, LeakyRelu
×7, Concat ×16, Resize ×8, Slice ×16, Cast ×16, Shape/Gather ×8, Mul ×8, Min/Max, Transpose ×2`. No
custom op, no `ConvTranspose`, no InstanceNorm; the `gen_pconv` ("partial conv") layers decompose to
`Conv`+`Slice`+`Concat`+`Mul`. The `Shape/Gather/Slice/Concat` are `Resize`'s dynamic-size math —
**static once the batch dim is pinned to 1**, so tract const-folds them.

## tract feasibility: ✅

Pin the batch dim and run:

```rust
let plan = tract_onnx::onnx()
    .model_for_path(weights)?
    .with_input_fact(0, f32::fact([1, 512, 512, 3]).into())?  // H/W/C already fixed → all-static
    .into_optimized()?
    .into_runnable()?;
let out = plan.run(tvec!(Tensor::from_shape(&[1,512,512,3], &tile_nhwc)?.into()))?;
let starless_tile = out[0].as_slice::<f32>()?;  // NHWC [1,512,512,3]
```

`into_optimized()` const-folds the dynamic shape machinery (because H/W are fixed), leaving a plain
conv U-Net. Runs CPU-only, multithreaded by tract.

## Pipeline in lumos (`star_removal.rs`)

`remove_stars(&stretched_image, &StarRemovalConfig{weights, stride})` →
`{ starless, stars }`:
1. tile the image into 512² windows at `stride` (last tile flush to the edge);
2. per tile: pack NHWC `[0,1]` (grayscale replicates to R=G=B), run the model, get the starless tile;
3. **feather-blend** overlaps (a ramped window, weight ∈ `[0.02, 1]`, accumulate `Σ out·w / Σ w`);
4. **starless** = the blended result; **stars** = `unscreen(original, starless)`.

It's a **display-domain** op — feed it the stretched (`[0,1]`) image, after `stretching` (and
typically before/around `local_contrast`/`hdr`). The classic starless workflow then is: remove stars
→ process the starless nebula hard → screen the stars back.

## Cost & caveats

- **CPU only, slow.** A StarNet U-Net at 512²×3 is heavy; a 24 MP frame is ~hundreds of tiles
  (minutes). Fine for a final-image step; not interactive. (The test runs a 1024² crop = 9 tiles.)
- **Preprocessing is the risk** — StarNet was trained on a particular normalization. We feed
  stretched `[0,1]`, which matches how the CLI is used (it takes stretched 8/16-bit TIFF/PNG); if a
  given model wants something else, output degrades. Validate visually.
- **Memory** — model ~131 MB + the tract plan; per-tile tensors are small.

## Status

Prototype: generic tract backend + StarNet2 tiling/blend, behind `--features ml`. Test
(`--features ml,real-data`, `STARNET2_ONNX=<path>`) runs it on a crop and writes
`test_output/star_removal/{input,starless,stars}.png`. The morphological (no-model, in-Rust)
star-removal alternative remains the license-free fallback — not built.
