# Simd

Vector kernels written once, for every CPU: the one crate of the workspace
that names a vector register. imaginarium and lumos build their kernels on it.

- **One kernel model: a safe body generic over an `Isa` token**,
  monomorphized per backend, as Google's Highway, under libjxl, and the Rust
  crate `pulp` do. Refused: a function per op and per tier, which writes each
  op once per architecture, holds `unsafe` in every kernel and needs a
  function-pointer table per op.
- **One logical width per type on every Isa**: `F32x8`, `F64x4`, `U8x32`,
  `U16x16`, `I32x8`, 256 bits, one register on AVX2 and two on NEON. Each
  tail, each reduction order and each crossover is then one choice. Refused:
  the native width per Isa, which lets a reduction or a tail give other bits
  on another CPU.
- **x86-64-v3 is the x86 baseline, as NEON is aarch64's**: the crate refuses
  an x86 build without AVX2 and FMA, a token needs no detection, and no
  `#[target_feature]` entry fences a kernel, so no op can fall out of line
  into code built without the features. `Portable` is dispatched only on
  another architecture. RHEL 10 sets the same baseline. Refused: run-time
  detection with a lane-loop fallback, a check per dispatch for a CPU from
  before 2013 that the shipped binary does not run on anyway.
- **An op is the unit of specialization; a kernel never is.** A composite op —
  over several vectors or a pixel layout — has a body in `Isa`, its definition
  in lane loops, which `Portable` takes; a backend overrides it where its
  instructions beat that, as NEON's structure stores, x86's byte shuffles and
  AVX2's gathers do, and the tests hold every override to `Portable`. Refused:
  per-ISA kernels outside the crate, which branch on the platform; generic
  kernels alone, which on x86 deinterleave through byte shuffles and measured
  up to 2.2× slower on a row in cache.
- **Every op is specified to the bit, and `Portable` is its model.** A
  float-to-integer round clamps and rounds to nearest, ties to even; `max` and
  `min` are a compare and a select, so a NaN and a signed zero decide alike on
  every Isa. A narrow takes lanes in range by contract, which `Portable`
  asserts: NEON's saturating narrows measured up to 1.7× slower than its plain
  ones.
- **A gather never reads outside its table.** A lane outside it reads zero;
  the hardware paths load whole lanes and send a vector with a lane past either
  end lane by lane, which only an image's first and last pixels do. A `u16` or
  `f32` pixel pair is one load and a transpose: AVX2's gather a channel
  measured no faster on Zen 3, and NEON has no gather.
- **A row's last partial group runs the kernel's own step**, from the row's
  last whole group, overlapping the one before, or padded in a row shorter
  than a group, into a scratch group whose new part is copied out; in place,
  padded alone, as an overlap would step twice. Refused: the scalar reference
  for the tail, which cost up to a fifth of a 6000-pixel row in cache; the
  overlapping group stored in place, which waits on the earlier store's cache
  miss and doubled an out-of-cache `u8 → u16` frame on an Apple core.
- **A kernel calls an op in place**, not through a closure it calls more than
  once, and names its vectors rather than indexing an array of them in a
  loop: a closure cannot carry `#[inline(always)]` and was left out of line,
  and an array of NEON's register pairs went through the stack with a
  `memcpy`; each measured two to four times slower on an M2.
- **A crate of its own, not a module of `common`**: only imaginarium and lumos
  use it, it holds the only arch `unsafe` of the workspace, and it has its own
  tests. Refused: a module of `common`, which puts some 4 000 lines on the
  path every crate waits for; `std::simd`, which needs a nightly toolchain;
  `pulp` or `wide`, whose ops are not specified to the bit.
- **Arch code lives behind this crate.** The root `clippy.toml` refuses the
  x86 and aarch64 register types everywhere but `simd`'s `backend` module.
