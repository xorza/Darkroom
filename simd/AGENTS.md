# Simd

Vector kernels written once, for every CPU: the one crate of the workspace that names a vector
register. imaginarium and lumos build their kernels on it.

- **One kernel model: a safe body generic over an `Isa` token**, monomorphized per backend, as
  Google's Highway and the Rust crate `pulp` do. Refused: a function per op and per tier, with
  `unsafe` in every kernel and a function-pointer table per op.
- **One logical width per type on every Isa**, 256 bits: one register on AVX2, two on NEON, so
  each tail, reduction order and crossover is one choice. Refused: the native width per Isa, which
  lets a reduction or a tail give other bits on another CPU.
- **x86-64-v3 is the x86 baseline, as NEON is aarch64's**: an x86 build without AVX2 and FMA is
  refused, a token needs no detection, and no `#[target_feature]` entry fences a kernel.
  `Portable` is dispatched only on another architecture. Refused: run-time detection with a
  fallback, a check per dispatch for CPUs the shipped binary does not run on.
- **An op is the unit of specialization; a kernel never is.** A composite op has a body in `Isa`,
  defined by lane loops that `Portable` takes; a backend overrides it where its instructions beat
  that, and the tests hold every override to `Portable`. Refused: per-ISA kernels outside the
  crate, which branch on the platform; generic kernels alone, which deinterleave slowly on x86.
- **Every op is specified to the bit, and `Portable` is its model**: a float-to-integer round
  clamps and rounds to nearest, ties to even; `max` and `min` are a compare and a select, so a NaN
  and a signed zero decide alike on every Isa. A narrow takes lanes in range by contract, which
  `Portable` asserts.
- **A kernel is bit-identical to its reference on one architecture; a new NaN's bits are not the
  same across two**: an op that makes a NaN sets its sign on x86 and clears it on aarch64
  ([Rust](https://doc.rust-lang.org/std/primitive.f32.html) leaves it non-deterministic).
  Refused: rewriting each NaN, a cost per op for bits no reader takes as a value.
- **A gather never reads outside its table**: a lane outside it reads zero.
- **A row's last partial group runs the kernel's own step**, overlapping the group before, or
  padded in a row shorter than a group, through a scratch group. Refused: the scalar reference
  for the tail, slow on short rows; the overlapping group stored in place, which waits on the
  earlier store.
- **A kernel calls an op in place and names its vectors**: a closure cannot carry
  `#[inline(always)]`, and an array of NEON register pairs goes through the stack.
- **A crate of its own, not a module of `common`**, as only imaginarium and lumos use it and it
  holds the workspace's only arch `unsafe`. Refused: `std::simd`, which needs nightly; `pulp` or
  `wide`, whose ops are not specified to the bit.
- **Arch code lives behind this crate**: the root `clippy.toml` refuses the register types
  everywhere but `simd`'s `backend` module.
