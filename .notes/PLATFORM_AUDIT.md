# Platform behavior audit

This is a temporary proposal. When a change lands, its lasting decision moves into the `AGENTS.md`
or the doc comment of the system that it governs. Then delete the change from this file.

**Status:** All implementation is done. Only the shortcut label decision and the lists below stay
here. These landed, and their decisions moved:

- The touchpad rule, to `darkroom/AGENTS.md`, "One scroll rule".
- The Windows wheel setting, to `palantir/AGENTS.md`, "A person's scroll setting moves a scroll,
  not a zoom".
- The Windows main thread stack, to `reserve_main_stack` in `app/build.rs`.
- The removal of Direct3D 12, to `palantir/AGENTS.md`, "Vulkan, Metal and GL; no Direct3D 12". It
  replaced the earlier decision to keep Direct3D 12 as a fallback.
- The bundled font names, to `hide_installed_bundled` in `palantir/src/text/font_scope.rs`.
- The foreign absolute paths, to `darkroom/AGENTS.md`, "A path in another system's absolute form
  stays as the file holds it".
- The LibRaw flag, to `libraw/build.rs`.
- The file watch, URL failure and camera clock corrections, to their doc comments.
- The NaN limit, to `simd/AGENTS.md`.
- The system math library, to `AGENTS.md`, "A transcendental float function goes through
  `common::Libm`", with the same line in `palantir/AGENTS.md`.

## Background

### Why this review

Both `AGENTS.md` and `palantir/AGENTS.md` state one rule: one behavior in every build and on every
platform. A difference stays only when it changes speed alone, or when the system decides it. A
person's settings and the capacity of a device are system decisions. A platform convention alone is
no reason for a difference.

Two open palantir issues appear on one CI OS only, so the rule needed a check. This review found
that neither issue is a behavior difference:

- `Staging::abort` is a race in wgpu-core 30. The Windows runner hit it, but the code that races is
  not specific to Windows.
- The uncommitted test GPU lease change in `palantir` ties the Ubuntu `SIGSEGV` to a second wgpu
  instance under lavapipe. This review did not reproduce that crash.

### Scope and method

The review covered the workspace crates and the submodules `palantir` and `fits-well`. It did not
review `libraw/LibRaw` as code, only how `libraw/build.rs` compiles it. It looked for these items:

- Platform branches: each `cfg` on an OS or an architecture, and each target-specific dependency.
- Input: the path from each OS event through `winit` and palantir to the Darkroom gestures.
- GPU: the adapter choice, the surface format, the color space, and the CI adapters.
- Fonts: how the window host and the headless hosts load faces.
- Files: paths in a document, directory order, file watches, and case rules.
- Numbers: calls into the system math library, compiler flags, and results that depend on the
  thread count.

After the findings, a second pass searched for the established practice on each open question. The
decisions below cite what it found. Where a search found only a secondary source, the text says so.

### Limits of the evidence

**The review ran no product code on any OS.** Each statement about Windows, Linux or macOS comes
from the source of this repository or of a dependency, at the version in `Cargo.lock`. The label
"Confirmed in source" does not mean that a person observed the behavior. Run each change on the OS
that it names before you close it.

### A correction

An earlier note said that `cfg(target_os` occurs only in `palantir/src/common/platform.rs` and
`darkroom/src/platform/mod.rs`. That note was wrong, because the search pattern was too narrow. See
"Checked, no difference found" for the full result.

## Summary

The platform branches are in good order. The behavior differences are in code that has no `cfg`.
No finding is a data-loss bug in a common workflow.

| Finding | Effect | Evidence | Decision |
|---|---|---|---|
| Shortcut labels follow a platform convention | `⌘S` against `Ctrl+S` | Confirmed in source | Keep `⌥⇧⌘S` on macOS |

## Decisions

### Shortcut labels follow a platform convention

**Where:** `Platform::labels_with_symbols` in `palantir/src/common/platform.rs`.

**Behavior:** macOS reads `⌥⇧⌘S`. Windows and Linux read `Ctrl+Shift+Alt+S`.

**Decision: keep `⌥⇧⌘S` on macOS.** It is the canonical form there.
[Qt's `NativeText`](https://doc.qt.io/qt-6.5/qkeysequence.html) shows Mac symbols, and palantir's
order follows [Apple's Style Guide](https://daringfireball.net/2026/03/modifier_key_order_for_keyboard_shortcuts):
Control, Option, Shift, Command. This is a deliberate exception to "a convention alone is no reason",
and it applies to a label only. The chord itself stays the same on every OS.

**Refused:** Names on every OS, such as `Cmd+Shift+Alt+S`.

## Accepted differences

Each of these is a system decision or changes speed alone. None needs a change.

| Difference | Why it stays |
|---|---|
| Cmd is the command key on macOS | The rule's one substitution |
| `⌥⇧⌘S` on macOS | The decision above |
| Option composes text on macOS, AltGr elsewhere | The keyboard layout decides what a key types |
| macOS turns Shift and wheel into a sideways scroll itself | The system does it, and palantir does the same elsewhere |
| Double-click time, reduced motion, reduced transparency | A person's settings, through `mundy` |
| The config and cache folders per OS | The system names them |
| `route_opened_documents` on macOS alone | Launch Services sends an event where the others use argv |
| Font fallback and locale in the window | Installed fonts are what the machine holds |
| Vsync, present mode and frame latency per backend | Speed alone |
| A Windows wheel notch scrolls 3 lines by default, and one line elsewhere | The Windows setting decides it. AppKit applies its own speed, and the X11 and Wayland protocols state none |
| `ScratchFile`, `replace` and `sync_parent` per OS | One contract, each with its own system calls |
| OpenMP in LibRaw depends on the toolchain | Speed alone, and `libraw::OPENMP` states it |
| Memory limits: cgroups on Linux, job objects on Windows | What a device can hold |
| Native dialog backends through `rfd` | The system's own dialog |

## Checked, no difference found

- **Platform branches.** `cfg` on an OS or an architecture occurs in these places alone:
  `common/src/platform/`, `darkroom/src/platform/`, `palantir/src/host/winit/mod.rs` (which picks
  `host/winit/platform/`), `palantir/src/common/platform.rs`, `simd`, and
  `palantir/src/primitives/packed/half_simd/`. `app/build.rs` reads the target through
  `CARGO_CFG_TARGET_OS`, and `libraw/shim/internal.cpp` has one `_WIN32` branch. Each is the one
  entity that owns its difference. `darkroom/src` never reads `PLATFORM`.
- **Metal color space.** wgpu-hal 30 leaves the layer's default for `Srgb`, and its comment says that
  the default treats contents as sRGB. palantir asks for `Srgb` on every OS.
- **Surface format.** `build_surface_config` takes the first sRGB format and refuses a surface with
  none. `capture.rs` handles the RGBA and BGRA orders.
- **Saved paths.** `SavedPaths` writes `/` on every OS, and `resolve` reads each part with the
  system's own separator.
- **Directory order.** `raw_files`, `stamp_dir` and `normalize_file_selection` each sort. The
  `read_dir` loop in `darkroom/src/driver/mod.rs` was not traced to its use.
- **Extension case.** `InputFormat` and `DocumentFormat::of_path` ignore case.
- **Line endings.** `.gitattributes` sets `eol=lf`, and `serde_format.rs` fixes RON's line ending.
- **Thread count.** Tests hold results across band counts: labeling strips, the detection filter,
  drizzle bands, and flat preparation on 1 and 7 threads. A search for a parallel float `sum` or
  `reduce` found only `imaginarium`'s test helper. Other parallel folds were not traced by hand.
- **Memory tiers.** `disk_and_memory_tiers_produce_identical_masters` holds the two storage tiers
  equal, so a machine's memory changes speed alone there.
- **Vector kernels.** `simd` holds each tier to its reference on the CPU that runs the test. CI runs
  x64 and aarch64.
- **FFI types.** Bindgen runs per target, and `image_data.rs` handles the sign of `c_char`.
- **The GPU backend of `imaginarium`.** Neither `lens` nor `darkroom` enables its `wgpu` feature, so
  the shipped editor does not run those shaders.

## CI coverage

Each lane proves less than its name suggests.

| Lane | CPU | GPU adapter now | Not covered |
|---|---|---|---|
| `ubuntu-26.04` | x64 | lavapipe, software Vulkan | Real GPUs, Wayland, X11 |
| `macos-26` | aarch64 | The runner's Metal | x64 macOS |
| `windows-2025` | x64 | lavapipe, software Vulkan | GL on Windows, and real GPUs |

These gaps hold on all three lanes:

- No test opens a native window. The one that does in `host/winit/tests.rs` is `#[ignore]`.
- `cargo visual` and `cargo real-data` do not run in CI.
- No lane compares its results with another lane, except that each compares `lumos` with one
  committed set of snapshots, to the bit.
- Input tests call `Translator` with a `Platform` value and a `winit` event that they build. They
  prove palantir's translation, but not what each OS sends through `winit`.

## Not verified

These items hold no decision, as they need a run on the OS that they name.

- **The ONNX Runtime nodes.** `lens` enables `lumos/ml`, and `Session::builder` uses the default CPU
  provider on every OS. The search found no ONNX Runtime statement on results across CPUs. MLAS keeps
  [separate kernels per instruction set](https://app.semanticdiff.com/gh/microsoft/onnxruntime/commit/a4e58e8feb375f266a4e80c669d7b467a11ea0a3),
  so results may not be bit-identical. The snapshots leave these nodes out.
- **The Linux file dialog with no backend.** `rfd` tries the portal, then `zenity`, and returns `None`
  when both fail (`xdg_desktop_portal.rs:105`). Darkroom cannot tell that from a cancel, so Open would
  do nothing. `rfd` logs an error.
- **The native dialog's block.** A dialog blocks the main thread on every OS. A compositor may mark
  the window as not responding on Linux.
- **The clipboard on Linux** after the app exits, and under a Wayland compositor with no data-control
  protocol. palantir falls back to its own memory buffer in that second case.
- **Fractional scale factors** on a real window on Windows and Linux.
- **IME behavior** on a real window on each OS.
- **Vulkan resize artifacts on Windows,** from a search summary that a direct search did not confirm,
  on a real GPU.
- **The GL fallback on Windows,** which no CI lane runs, on a machine with no Vulkan driver.
- **Packaging and installers**, which this review did not read.
- **`statrs` and LibRaw** call their own math, so a `lumos` snapshot may show a last-bit difference on
  x64 that `Libm` cannot remove.
