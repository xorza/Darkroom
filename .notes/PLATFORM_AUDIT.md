# Platform behavior audit

This is a temporary proposal. When a change lands, its lasting decision moves into the `AGENTS.md`
or the doc comment of the system that it governs. Then delete the change from this file.

**Status:** Each open finding has a decision. These landed, and their decisions moved:

- The touchpad rule, to `darkroom/AGENTS.md`, "One scroll rule".
- The Windows wheel setting, to `palantir/AGENTS.md`, "A person's scroll setting moves a scroll,
  not a zoom".
- The Windows main thread stack, to `reserve_main_stack` in `app/build.rs`.
- The removal of Direct3D 12, to `palantir/AGENTS.md`, "Vulkan, Metal and GL; no Direct3D 12". It
  replaced the earlier decision to keep Direct3D 12 as a fallback.
- The bundled font names, to `hide_installed_bundled` in `palantir/src/text/font_scope.rs`.
- The foreign absolute paths, to `darkroom/AGENTS.md`, "A path in another system's absolute form
  stays as the file holds it".

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
| System math library in results | The last bit of a result differs by OS | Rust docs state it, size not measured | `ops` module over `libm`, with a digest test |
| LibRaw can fuse float operations on aarch64 | LibRaw output can differ by architecture | Compiler docs state it, not measured | `-ffp-contract=off` |
| File watch fires on metadata on Windows | A doc promise fails on one OS | Confirmed in source | Correct the docs |
| A failed URL open reports on Windows only | A failure is quiet on Linux and macOS | Confirmed in source | Log the exit status |
| Shortcut labels follow a platform convention | `⌘S` against `Ctrl+S` | Confirmed in source | Keep `⌥⇧⌘S` on macOS |
| Camera clock depends on the machine's zone | One wall-clock hour can shift | Derived from source | Correct the doc |
| A new NaN has other bits on x86 and aarch64 | A scope limit on "bit-identical" | Rust docs state it | State the limit |

## Approved dependencies

This approval applies only to the change below.

- **`libm` 0.2**, direct, in each project that gets an `ops` module. `Cargo.lock` and
  `palantir/Cargo.lock` already hold it through other crates.

## Implementation order

1. The LibRaw flag, then the `ops` modules with their digest test.
2. The doc corrections, in any order.

## Decisions

### System math library in results

**Where:** `powf`, `exp`, `ln`, `sin`, `cos`, `atan2`, `hypot`, `cbrt` and the like, in code that
produces a result. A text search outside tests and benches found about 120 calls in `lumos`, 160 in
`fits-well`, 35 in `palantir` and 9 in `darkroom`. The count is rough, as it also holds `powi` and
calls named `log`. `imaginarium` has none, and `simd` has one `powi`.

**Behavior:** The Rust docs state that the precision of these functions "varies by platform, Rust
version, and can even differ within the same execution"
([f32](https://doc.rust-lang.org/std/primitive.f32.html)). The call goes to the libm of the system.

**The project already decided this once.** `palantir/src/primitives/paint/color/oklab.rs:87` refuses
`f32::cbrt`, "whose last bit differs (MSVC's against Apple's and glibc's), so a motion, a gradient
and a golden would differ by platform". But `animation/spring.rs`, `shape/paint/blur_plan`,
`shape/paint/lens_optics`, the gradient geometry and `color/okhsv` still call libm.

**Effect in `lumos`:** The same frames can give other bits per OS. A last-bit change is small. But
star detection and `registration/ransac` compare against thresholds, so a change can in principle
move a decision. The size of this effect is not measured. No CI lane compares a result with another
lane, and the palantir goldens are local and tied to one adapter.

**Decision: one `ops` module over the `libm` crate, refused direct calls, and a digest test.** This is
the practice of [Bevy's `bevy_math::ops`](https://raw.githubusercontent.com/bevyengine/bevy/main/crates/bevy_math/src/ops.rs)
with its [`clippy.toml`](https://raw.githubusercontent.com/bevyengine/bevy/main/crates/bevy_math/clippy.toml),
and [Rapier's `enhanced-determinism`](https://rapier.rs/docs/user_guides/rust/determinism/) also goes
through libm. Bevy makes it a feature. Here it is the one behavior, with no feature.

**What follows from it:**

- **One module per project.** `common` serves the workspace crates. `palantir` and `fits-well` each
  hold their own, as a submodule stays standalone.
- **`clippy.toml` refuses the direct methods** on `f32` and `f64`: `powf`, `powi`, `log`, `ln`, `log2`,
  `log10`, `exp`, `exp2`, `exp_m1`, `ln_1p`, `cbrt`, `hypot`, `sin`, `cos`, `tan`, `asin`, `acos`,
  `atan`, `atan2`, `sin_cos`, `sinh`, `cosh`, `tanh`, `asinh`, `acosh`, `atanh`. Tests stay under the
  same rule.
- **`powi` has no `libm` form,** as Bevy notes. About 56 calls exist outside tests. A small power
  becomes `squared` or `cubed`, and a power of two becomes an exact scale.
- **`glam` calls std in its own methods** unless its `libm` feature is on, so the feature turns on
  wherever `glam` serves a result.
- **`oklab::cbrt` stays.** It rounds to the nearest `f32`, which `libm::cbrtf` does not promise.
- **The digest test** runs one synthetic scene through the `lumos` pipeline, with its digest committed,
  so that all three CI lanes must agree. It leaves out the ONNX nodes. A Rust or `libm` update can
  move the digest, which is a fact to learn.
- **Bench the hot kernels** before and after, as `libm` can be slower than a system libm. Start with
  `lumos/src/image_ops/stretch` and `lumos/src/math/pixel_quadrature`.

**Refused:**

- The digest test alone: a pass on one scene proves little about the others, and the known cause stays.
- Own functions for each call, as `oklab::cbrt` does: many functions, each with its own accuracy tests.
- An exception in the rule for the last bit: it conflicts with the `oklab::cbrt` decision, and a
  threshold can still move a decision.

### LibRaw can fuse float operations on aarch64

**Where:** `libraw/build.rs`, which sets no `-ffp-contract`.

**Behavior:** Clang's default is `on`, which fuses within one statement
([Clang](https://clang.llvm.org/docs/UsersManual.html)). GCC's default for this build is `fast`,
which fuses across statements ([GCC](https://gcc.gnu.org/onlinedocs/gcc/Optimize-Options.html)).
A fused operation needs FMA in the target. aarch64 has it in its baseline. The `cc` crate does not
pass the workspace's `target-cpu=x86-64-v3` to the compiler, so the x64 build of LibRaw has no FMA.

**Effect:** `UnpackedRaw::processed_by_libraw` runs for a sensor that `lumos` does not demosaic and
for a chosen LibRaw quality. Its float stages can round differently on aarch64 and x64. The 16-bit
output hides most of that. This review did not measure it, and no CI test runs it on real data.

**Decision: add `-ffp-contract=off` through `flag_if_supported` in `libraw/build.rs`.** It touches no
upstream source. This is the known fix: an
[LLVM issue](https://www.github.com/llvm/llvm-project/issues/91824) and a
[GCC report](https://mail-archive.com/gcc-patches@gcc.gnu.org/msg303356.html) both resolve an aarch64
difference with it, [GSL's RPM](https://git.almalinux.org/rpms/gsl/commit/1fdd10c1a061741349e429c93679ad5e487d6ad7)
sets it on aarch64, and [Blender](https://developer.blender.org/rB406d74769597e318117ad5b3837e7283d9fd2aa5)
turned contraction off for the same reason.

**Cost:** A possible small slowdown of LibRaw's float code on aarch64. `flag_if_supported` skips the
flag for MSVC. LibRaw's own libm calls stay, so this removes one cause of a difference and not all of
them.

### File watch fires on metadata on Windows

**Where:** `lens/src/fs_watch/mod.rs:125`.

**Behavior:** The node's description promises to ignore metadata-only events, and
`is_content_change` keeps `Modify(Any)`. `notify` 8 asks Windows for attribute and security changes
(`windows.rs:284`) and reports each as `Modify(Any)` (`windows.rs:449`). inotify and FSEvents report
them as `Modify(Metadata)`, which the filter drops.

**The comment names the wrong backend.** It says that FSEvents reports a write as `Modify(Any)`. In
`notify` 8 the `fsevent` backend sends no `Modify(Any)`, and it reports a write as `Modify(Data)`.
Windows is the coarse one.

**Decision: correct the comment, and state the Windows behavior in the node's description.** The cost
of a false fire is one extra run, and `notify` gives no way to tell the two events apart on Windows.

### A failed URL open reports on Windows only

**Where:** `open_url` in `common/src/platform/`.

**Behavior:** Windows returns the failure of `ShellExecuteW`. Linux and macOS start `xdg-open` or
`open` through `spawn_detached`, whose reaper thread drops the exit status. So only a missing program
reports on those two systems. The caller in `darkroom/src/gui/pane/preferences.rs:367` logs an error.

**Decision: the reaper logs a nonzero exit status.** The log then reads the same on all three systems.
The caller does not wait for the exit, as the call must not block the frame.

### Shortcut labels follow a platform convention

**Where:** `Platform::labels_with_symbols` in `palantir/src/common/platform.rs`.

**Behavior:** macOS reads `⌥⇧⌘S`. Windows and Linux read `Ctrl+Shift+Alt+S`.

**Decision: keep `⌥⇧⌘S` on macOS.** It is the canonical form there.
[Qt's `NativeText`](https://doc.qt.io/qt-6.5/qkeysequence.html) shows Mac symbols, and palantir's
order follows [Apple's Style Guide](https://daringfireball.net/2026/03/modifier_key_order_for_keyboard_shortcuts):
Control, Option, Shift, Command. This is a deliberate exception to "a convention alone is no reason",
and it applies to a label only. The chord itself stays the same on every OS.

**Refused:** Names on every OS, such as `Cmd+Shift+Alt+S`.

### Camera clock depends on the machine's zone

**Where:** `Libraw::camera_clock` in `libraw/src/libraw.rs:210`.

**Behavior:** LibRaw converts the camera's text through `mktime`, and the shim converts it back
through `localtime`. The doc says that this returns the camera's own clock. A wall-clock time inside
the machine zone's spring gap does not exist there, so `mktime` moves it. The value reaches the
image's `date_local` metadata.

**Decision: correct the doc comment, and accept the gap.** A fix needs a parse of each format that
LibRaw reads. The value is metadata, and no pixel depends on it.

### A new NaN has other bits on x86 and aarch64

**Where:** `simd/AGENTS.md`, "Every op is specified to the bit".

**Behavior:** An operation that makes a new NaN, such as `0.0 / 0.0`, gives a set sign bit on x86 and
a clear one on aarch64. The [Rust docs](https://doc.rust-lang.org/std/primitive.f32.html) state that
this sign is non-deterministic. The scalar reference runs on the same CPU as the kernel, so the tests
pass, and they cannot see this.

**Decision: state the limit in `simd/AGENTS.md`.** A kernel is bit-identical to its reference on one
architecture, and NaN bits are not the same across two. Change nothing else. The digest test must not
hash a NaN's bits.

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
- No lane compares its results with another lane. The digest test closes this gap for `lumos`.
- Input tests call `Translator` with a `Platform` value and a `winit` event that they build. They
  prove palantir's translation, but not what each OS sends through `winit`.

## Not verified

These items hold no decision, as they need a run on the OS that they name.

- **The ONNX Runtime nodes.** `lens` enables `lumos/ml`, and `Session::builder` uses the default CPU
  provider on every OS. The search found no ONNX Runtime statement on results across CPUs. MLAS keeps
  [separate kernels per instruction set](https://app.semanticdiff.com/gh/microsoft/onnxruntime/commit/a4e58e8feb375f266a4e80c669d7b467a11ea0a3),
  so results may not be bit-identical. The digest test leaves these nodes out.
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
