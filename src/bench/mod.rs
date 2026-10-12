//! The criterion benches, which `benches/imaginarium.rs` runs: `./x.py bench`.
//!
//! Two kinds, and every case is one. An `atomic` case times one kernel on the unit dispatch hands
//! it, once for each variant: the scalar reference, and the kernel compiled for each tier this CPU
//! has. A row kernel gets one row of the standard frame; transform's kernel, which takes an
//! image, a 512 × 512 tile. An `integration` case times a public entry point on the standard
//! frame, on each backend it has, as a caller pays: an allocation the API makes, and on the GPU
//! the upload and the readback.
//!
//! A case's id is `<kind>/<group>/<backend>/<rest…>`, with the same depth across a group: the
//! group is the operation, named the same at both kinds, and the backend, `cpu` or `gpu`, comes
//! first in every case, so `'/cpu/'` selects every case that opens no device and `'/gpu/'` every
//! case that does. The rest is the variant (a tier) where a group has several, then the input.
//! Each case states its throughput in pixels and in the pixel bytes it reads. A case builds its
//! input in its own closure, from the scene's fixed seeds, so a filtered run builds only what it
//! times.

pub(crate) mod fixture;
pub(crate) mod scene;

use criterion::Criterion;

use crate::bench::fixture::Backends;
use crate::image;
use crate::image::conversion;
use crate::ops::{blend, contrast_brightness, preview, transform};

/// Every case, atomic first. The host's adapters are listed once here; no device opens.
pub fn run(c: &mut Criterion) {
    let backends = Backends::probe();

    conversion::bench::atomic(c);
    contrast_brightness::bench::atomic(c);
    blend::bench::atomic(c);
    transform::bench::atomic(c);

    conversion::bench::integration(c);
    contrast_brightness::bench::integration(c, &backends);
    blend::bench::integration(c, &backends);
    transform::bench::integration(c, &backends);
    preview::bench::integration(c);
    image::bench::integration(c);
}
