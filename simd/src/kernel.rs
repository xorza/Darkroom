//! [`Kernel`]: a vector kernel, one body for every Isa.

use crate::isa::Isa;
use crate::isa_tier::IsaTier;

/// A vector kernel: one body, generic over the Isa it runs on.
pub trait Kernel: Sized {
    type Output;

    /// The kernel on `isa`. Every implementation is `#[inline(always)]`, and so is everything it
    /// calls (see the crate documentation).
    fn run<S: Isa>(self, isa: S) -> Self::Output;

    /// The kernel on the widest Isa this CPU has.
    #[inline]
    fn dispatch(self) -> Self::Output {
        IsaTier::widest().run(self)
    }
}
