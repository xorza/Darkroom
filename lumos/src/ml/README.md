# ML filters (ONNX backend)

Optional ML-based filters behind the **`ml`** cargo feature, run through **`ort`** (ONNX Runtime).
**lumos bundles no model weights** — each filter loads a `.onnx` the *caller supplies*.

Currently: **`star_removal`** (StarNet-style) and **`denoise`** (DeepSNR-style), both on the shared
`backend` (`ml/backend.rs`): load model → overlapping 512² tiles → feather-blend.

## Why `ort` and not pure-Rust `tract`

We prototyped on **tract** (Sonos, pure-Rust, no C++ dep) and it ran **StarNet2** fine. But **DeepSNR**
uses `GatherND` / `ScatterND` / `If` (advanced indexing + control flow) that tract's optimizer can't
analyse — `into_optimized()` fails with *"Failed analyse … GatherNd"*. ort wraps the full ONNX Runtime
op set, so it loads both models unchanged. The trade-off: ort pulls in the **native onnxruntime**
library (downloaded at build time) instead of staying pure-Rust. Bonus: the heavy compute lives in the
precompiled native lib, so **inference is fast even in debug builds** (no profile gymnastics).

## Why caller-supplied weights (the licensing reality)

The strong astro CNNs are not redistributable:

- **StarXTerminator / NoiseXTerminator / BlurXTerminator** — commercial; EULA forbids use outside the
  paid plugin. Off the table.
- **StarNet2 (v2.5.2)** — `LICENSE.txt` is **non-transferable**, "solely for astrophotography image
  processing", and forbids using the software/outputs "to create any commercial software, including
  for training neural networks or other image processing systems."

→ The backend is **generic**: lumos provides the ort runner + tiling; the user points it at their own
legally-obtained ONNX (`TiledOnnxConfig::weights`; the tests read `STARNET2_ONNX` / `DEEPSNR_ONNX`,
defaulting to gitignored files in `test_data/`). lumos ships **no model**, staying clean of the license.

## Model contracts (verified by inspecting the ONNX)

Both are `tf2onnx`, opset 17, NHWC, fixed 512×512×3, batch dynamic:

| | StarNet2 (`x:0` → `Identity:0`) | DeepSNR v2 (`intro:0` → `Identity:0`) |
|---|---|---|
| Size / output | ~131 MB → **starless** image directly | ~111 MB → **denoised** image directly |
| Input | `[batch,512,512,3]` f32, stretched `[0,1]` | `[batch,512,512,3]` f32, stretched `[0,1]` |
| Architecture | conv U-Net (`gen_pconv` partial-conv → Conv/Slice/Concat/Mul, `Resize` upsample) | transformer-ish: `Erf` (GELU), squeeze-excite (`GlobalAveragePool`+`Sigmoid`+`Mul`), **`If`/`GatherND`/`ScatterND`** |
| tract? | ✅ loads | ❌ `GatherND` unsupported → ort |

Neither model stretches internally (no `Exp`/`Log`/`Pow`), so feed **stretched display data in
`[0,1]`** — run these **after** the stretch.

## The ort backend (`ml/backend.rs`)

```rust
let mut session = Session::builder()?.commit_from_file(weights)?;
let tensor = Tensor::from_array(([1, 512, 512, 3], tile_nhwc))?;   // [0,1] NHWC, grayscale → R=G=B
let outputs = session.run(ort::inputs![tensor])?;
let (_shape, out): (_, &[f32]) = outputs[0].try_extract_tensor::<f32>()?; // NHWC [1,512,512,3]
```

`run_tiled` does this over 512² tiles at `stride` (default 256, last flush to the edge) and
**feather-blends** overlaps (ramped window, weight ∈ `[0.02, 1]`, `Σ out·w / Σ w`), returning an
`AstroImage` with the input's channel count. The two filters are thin wrappers:

- **`remove_stars`** → `run_tiled` = starless; `stars = unscreen(original, starless)` (screen inverse).
- **`ml_denoise`** → `run_tiled` = the denoised image.

Both are **display-domain** — feed the stretched `[0,1]` image, after `stretching` (around
`local_contrast`/`hdr`). The starless workflow: remove stars → process the starless nebula hard →
screen the stars back.

## Cost & caveats

- **Speed** — onnxruntime CPU inference. Measured on a 10-core machine: the full 6032×4028 frame
  (345 tiles at stride 256) takes **~60 s sequentially** (~175 ms/tile); a 1024² crop (9 tiles) runs in
  seconds. The tile loop is **sequential by design** — these nets are memory-bandwidth-bound (~125 MB
  of weights streamed per tile), so ORT's intra-op threads already saturate the bus (1→10 threads
  scales only ~1.9×). Running tiles concurrently with one `Session` per worker was measured **~2×
  slower** (≈125 s, 10 workers) and **exhausted RAM** — each Session holds its own model copy plus a
  non-shrinking activation arena. So: do **not** wrap the tile loop in rayon. (`ml_perf.rs` times it.)
- **Preprocessing is the risk** — these nets were trained on a particular normalization. We feed
  stretched `[0,1]`, matching how the CLIs are used (stretched 8/16-bit TIFF/PNG). Validate visually.
- **Native dep** — ort downloads onnxruntime at build time (needs network once).

## Status

Prototype: generic ort backend + tiling/blend behind `--features ml`, with `star_removal` (StarNet2)
and `denoise` (DeepSNR) wrappers. Tests (`--features ml,real-data`) run each on a 1024² crop and write
`test_output/{star_removal,ml_denoise}/*.png`; they skip cleanly if the (gitignored) weights are
absent. Full-image processing works (see `ml_perf.rs`); tile-loop parallelism was investigated and
rejected (above). Open: starless-workflow helper; the license-free classical morphological
star-removal fallback.
