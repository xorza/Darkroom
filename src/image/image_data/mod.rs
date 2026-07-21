//! The two image-data layouts — a mutually-convertible pair, both carrying
//! `N` (channel count) and `T` (element type) in the type, each with a
//! runtime-erased enum over the 9 shipping formats:
//!
//! - [`interleaved`] — `ImageData<N, T>` = `Buffer2<[T; N]>` (`RGBRGB…`), the
//!   GPU/file-compatible layout that backs [`Image`](super::Image).
//! - [`deinterleaved`] — `DeinterleavedImageData<N, T>` = one `Buffer2<T>` per
//!   channel (`RRR…GGG…BBB…`), useful for per-channel CPU work and image-layout
//!   conversion.
//!
//! The erased forms (`AnyImageData` / `AnyDeinterleavedImageData`) are meant to
//! convert by transpose (interleave / deinterleave).

pub(crate) mod deinterleaved;
pub(crate) mod interleaved;
