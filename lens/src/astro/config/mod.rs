//! Which configs get a builder node, and the projections for the ones the
//! editor cannot edit in their own shape. See [`processing`] for the split.

pub(crate) mod preset;
pub(crate) mod processing;
pub(crate) mod stacking;

use scenarium::Library;

use lumos::{Denoise, ExtractBackground, Hdr, LocalContrast};

use crate::astro::config::processing::{ScnrKnobs, StretchKnobs};
use crate::astro::config::stacking::{CombineKnobs, DetectionKnobs, RegistrationKnobs};
use crate::config_node::add_config_builder;

pub(crate) fn register_builders(library: &mut Library) {
    add_config_builder::<ExtractBackground>(
        library,
        "9cda0462-1b8e-4c50-83d6-4db470df22d9",
        "Build Background Config",
        "Builds a detailed background-extraction config",
    );
    add_config_builder::<DetectionKnobs>(
        library,
        "6c6f92e7-0f74-454c-acc4-68691cb8462f",
        "Build Detection Config",
        "Builds a detailed star-detection config",
    );
    add_config_builder::<RegistrationKnobs>(
        library,
        "adf216fe-baa9-4abd-8c4a-bfb98bb60fbc",
        "Build Registration Config",
        "Builds a detailed registration config",
    );
    add_config_builder::<CombineKnobs>(
        library,
        "05313ceb-a3b2-4488-92af-c9e228bb1789",
        "Build Combine Config",
        "Builds a detailed frame-combination config",
    );
    add_config_builder::<Denoise>(
        library,
        "77693298-3531-4858-89ce-03cb347dc3f2",
        "Build Denoise Config",
        "Builds a detailed wavelet-denoise config",
    );
    add_config_builder::<Hdr>(
        library,
        "dc82d7a9-b7a7-460b-a86d-5dc9055e0d18",
        "Build HDR Config",
        "Builds a detailed HDR dynamic-range-compression config",
    );
    add_config_builder::<LocalContrast>(
        library,
        "f9ebdedf-38e3-4a74-8c74-eb207903d327",
        "Build Local Contrast Config",
        "Builds a detailed local-contrast config",
    );
    add_config_builder::<StretchKnobs>(
        library,
        "82f271d4-d047-459a-83aa-0bf8288787cf",
        "Build Stretch Config",
        "Builds a detailed display-stretch config",
    );
    add_config_builder::<ScnrKnobs>(
        library,
        "d07742d1-4469-4739-b2ff-78b4dcf64132",
        "Build SCNR Config",
        "Builds a detailed SCNR (green-removal) config",
    );
}
