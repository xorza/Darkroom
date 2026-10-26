//! Vector kernels written once, for every CPU.
//!
//! A kernel implements [`Kernel`]. Its body is generic over an [`Isa`] — a zero-sized token for an
//! instruction set — and [`Kernel::dispatch`] runs it on [`IsaTier::widest`]: its architecture's
//! baseline, `Avx2Fma` (x86-64-v3) on `x86_64` and `Neon` on aarch64, or `Portable` lane loops on
//! any other CPU. No CPU is checked at run time: the x86 build refuses to compile without AVX2 and
//! FMA.
//!
//! Every Isa has the same logical width — [`F32_LANES`] f32 lanes, [`F64_LANES`] f64 lanes,
//! [`U8_LANES`] bytes — whatever its registers hold: AVX2 keeps a vector in one register, NEON in
//! two. The lane split, the tails, the reduction order and every crossover are therefore one
//! choice rather than one per CPU, and every op is specified to the bit. A kernel built from these
//! ops alone computes the same values on every Isa — the same bits, save a NaN's sign and payload,
//! which IEEE 754 leaves to the hardware.
//!
//! An op is the one place an Isa differs. A vector type's ops are each Isa's own instructions. An
//! op over several vectors or a pixel layout has a body in [`Isa`], its definition in plain lane
//! loops, which `Portable` takes as it is; a hardware Isa overrides it where its instructions beat
//! that, as NEON's structure loads or x86's byte shuffles do. The tests hold each hardware Isa to
//! `Portable` op by op, so a kernel never branches on the Isa, and an override never changes a
//! result.
//!
//! Kernels hold no `unsafe`: loads take a `&[f32; 8]` from `as_chunks` or a checked slice.
//! Everything [`Kernel::run`] reaches is `#[inline(always)]`, as a kernel is monomorphized in its
//! caller's crate and an op left out of line costs a call per vector. A closure cannot carry the
//! attribute, so a kernel calls a gather or a load in place, not through a closure it calls more
//! than once, which the compiler may leave out of line.

#[expect(
    clippy::disallowed_types,
    reason = "the backends are the one place the workspace names a vector register"
)]
mod backend;
mod f32x8;
mod f64x4;
mod frexp;
mod full_scale;
mod halves;
mod i32x8;
mod isa;
mod isa_tier;
mod kernel;
mod mask8;
mod math;
mod u16x16;
mod u8x32;

#[cfg(target_arch = "x86_64")]
pub use crate::backend::avx2_fma::{
    Avx2Fma, f32x8::Avx2F32, f64x4::Avx2F64, i32x8::Avx2I32, mask8::Avx2Mask, u8x32::Avx2U8,
    u16x16::Avx2U16,
};
#[cfg(target_arch = "aarch64")]
pub use crate::backend::neon::{
    Neon, f32x8::NeonF32, f64x4::NeonF64, i32x8::NeonI32, mask8::NeonMask, u8x32::NeonU8,
    u16x16::NeonU16,
};
#[cfg(any(
    feature = "internals",
    not(any(target_arch = "x86_64", target_arch = "aarch64"))
))]
pub use crate::backend::portable::{
    Portable, f32x8::PortableF32, f64x4::PortableF64, i32x8::PortableI32, mask8::PortableMask,
    u8x32::PortableU8, u16x16::PortableU16,
};
pub use crate::f32x8::{F32_LANES, F32x8};
pub use crate::f64x4::{F64_LANES, F64x4};
pub use crate::frexp::Frexp;
pub use crate::full_scale::FullScale;
pub use crate::halves::Halves;
pub use crate::i32x8::{I32_LANES, I32x8};
pub use crate::isa::{Isa, Mask};
pub use crate::isa_tier::IsaTier;
pub use crate::kernel::Kernel;
pub use crate::mask8::Mask8;
pub use crate::math::{ASINH_LOG_FROM, Math};
pub use crate::u8x32::{U8_LANES, U8x32};
pub use crate::u16x16::{U16_LANES, U16x16};

#[cfg(test)]
mod tests;
