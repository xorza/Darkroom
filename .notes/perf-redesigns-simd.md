# simd: redesigns for the user's call

## NEON mixes RGBA `f32` affine taps in pixel layout

- **Idea.** A NEON register holds one RGBA `f32` pixel exactly. For the affine warp's bilinear
  RGBA `f32` kernel, NEON loads each lane's four taps as pixels, mixes them with the lane's own
  fractions (`vmulq_n_f32`, separate multiply and add, as the reference rounds), and stores the
  pixel as it is: no transpose into channels and none back.
- **Evidence.** The current kernel, one shape for every Isa, pair-gathers and transposes twice
  per row of taps and transposes back at the store; on the M2 it is 1.06–1.12 × master's per-pixel
  NEON kernel in cache. Spike against base `94dbf4eee`, ABBA, three rounds on the M2:
  `atomic/affine_warp/cpu/neon/bilinear/rgba_f32` 0.816 (250.8 → 204.7 µs), below master's
  245 µs; `integration/.../rgba_f32` 0.953 (43.8 → 41.7 ms), within the Mac's full-frame noise.
- **Target shape.** A composite op in `simd`, for example `Isa::bilinear_rgba_f32(table, starts,
  valid, fx, fy, out: &mut [f32; 32])`: the default body is today's pair gathers, mix and
  interleaved store, which `Portable` and AVX2 take; NEON overrides it in pixel layout. The
  kernel calls the op for RGBA `f32` and keeps its shape for the other formats. RGBA `u16` could
  follow with a widening load.
- **Against the design.** `simd/AGENTS.md` says "an op is the unit of specialization; a kernel
  never is". This op is most of one kernel's step, so it bends that line: the reason to take it
  or leave it is yours.
- **What the spike skipped.** It sits in imaginarium under `cfg(target_arch = "aarch64")` with
  raw intrinsics, not as an op. It does not guard the image's left edge: a pair starting before
  the table (`x₀ = −1`) overflows `usize` in a debug build, and
  `test_rgba_alpha_is_interpolated` fails there; release builds wrap and read the right pixel.
  No `Portable` model, no op test.
- **Cost of the real change.** One op with a default body and a NEON override (about 80 lines),
  its cross-check test against `Portable`, the kernel's RGBA `f32` arm, and the edge guard.
- **Spike branch.** `spike-neon-pixel-mix` (local), commit `2b9bd8703`, worktree
  `../darkroom-perf/spike`.
