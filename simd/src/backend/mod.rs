//! The backends, one per instruction set, each picked here by the architecture that has it.
//!
//! An architecture's baseline is its one hardware Isa: NEON on aarch64, AVX2 with FMA
//! (x86-64-v3) on x86, which `.cargo/config.toml` builds for. No CPU is checked at run time, and
//! `Portable` is dispatched only on another architecture.

#[cfg(all(
    target_arch = "x86_64",
    not(all(target_feature = "avx2", target_feature = "fma"))
))]
compile_error!(
    "the x86 baseline is x86-64-v3: build with `-C target-cpu=x86-64-v3`, as `.cargo/config.toml` does"
);

#[cfg(target_arch = "x86_64")]
pub(crate) mod avx2_fma;
#[cfg(target_arch = "aarch64")]
pub(crate) mod neon;
#[cfg(any(
    feature = "internals",
    not(any(target_arch = "x86_64", target_arch = "aarch64"))
))]
pub(crate) mod portable;
