//! Which configs get a builder node, and the projections for the ones the
//! editor cannot edit in their own shape. See [`processing`] for the split.

pub(crate) mod preset;
pub(crate) mod processing;
pub(crate) mod stacking;

use std::fmt;

use common::Introspect;
use lumos::{Denoise, ExtractBackground, Hdr, LocalContrast, Scnr};
use scenarium::{FuncId, Library};

use crate::astro::config::processing::StretchKnobs;
use crate::astro::config::stacking::{CombineKnobs, DetectionKnobs, RegistrationKnobs};
use crate::config_node::ConfigValue;
use crate::node_spec::NodeSpec;

const BUILD_BACKGROUND_CONFIG_FUNC_ID: FuncId =
    FuncId::literal("9cda0462-1b8e-4c50-83d6-4db470df22d9");
const BUILD_DETECTION_CONFIG_FUNC_ID: FuncId =
    FuncId::literal("6c6f92e7-0f74-454c-acc4-68691cb8462f");
const BUILD_REGISTRATION_CONFIG_FUNC_ID: FuncId =
    FuncId::literal("adf216fe-baa9-4abd-8c4a-bfb98bb60fbc");
const BUILD_COMBINE_CONFIG_FUNC_ID: FuncId =
    FuncId::literal("05313ceb-a3b2-4488-92af-c9e228bb1789");
const BUILD_DENOISE_CONFIG_FUNC_ID: FuncId =
    FuncId::literal("77693298-3531-4858-89ce-03cb347dc3f2");
const BUILD_HDR_CONFIG_FUNC_ID: FuncId = FuncId::literal("dc82d7a9-b7a7-460b-a86d-5dc9055e0d18");
const BUILD_LOCAL_CONTRAST_CONFIG_FUNC_ID: FuncId =
    FuncId::literal("f9ebdedf-38e3-4a74-8c74-eb207903d327");
const BUILD_STRETCH_CONFIG_FUNC_ID: FuncId =
    FuncId::literal("82f271d4-d047-459a-83aa-0bf8288787cf");
const BUILD_SCNR_CONFIG_FUNC_ID: FuncId = FuncId::literal("d07742d1-4469-4739-b2ff-78b4dcf64132");

pub(crate) fn register_builders(library: &mut Library) {
    add_builder::<ExtractBackground>(
        library,
        NodeSpec {
            id: BUILD_BACKGROUND_CONFIG_FUNC_ID,
            name: "Build Background Config",
            description: "Builds a detailed background-extraction config",
        },
    );
    add_builder::<DetectionKnobs>(
        library,
        NodeSpec {
            id: BUILD_DETECTION_CONFIG_FUNC_ID,
            name: "Build Detection Config",
            description: "Builds a detailed star-detection config",
        },
    );
    add_builder::<RegistrationKnobs>(
        library,
        NodeSpec {
            id: BUILD_REGISTRATION_CONFIG_FUNC_ID,
            name: "Build Registration Config",
            description: "Builds a detailed registration config",
        },
    );
    add_builder::<CombineKnobs>(
        library,
        NodeSpec {
            id: BUILD_COMBINE_CONFIG_FUNC_ID,
            name: "Build Combine Config",
            description: "Builds a detailed frame-combination config",
        },
    );
    add_builder::<Denoise>(
        library,
        NodeSpec {
            id: BUILD_DENOISE_CONFIG_FUNC_ID,
            name: "Build Denoise Config",
            description: "Builds a detailed wavelet-denoise config",
        },
    );
    add_builder::<Hdr>(
        library,
        NodeSpec {
            id: BUILD_HDR_CONFIG_FUNC_ID,
            name: "Build HDR Config",
            description: "Builds a detailed HDR dynamic-range-compression config",
        },
    );
    add_builder::<LocalContrast>(
        library,
        NodeSpec {
            id: BUILD_LOCAL_CONTRAST_CONFIG_FUNC_ID,
            name: "Build Local Contrast Config",
            description: "Builds a detailed local-contrast config",
        },
    );
    add_builder::<StretchKnobs>(
        library,
        NodeSpec {
            id: BUILD_STRETCH_CONFIG_FUNC_ID,
            name: "Build Stretch Config",
            description: "Builds a detailed display-stretch config",
        },
    );
    add_builder::<Scnr>(
        library,
        NodeSpec {
            id: BUILD_SCNR_CONFIG_FUNC_ID,
            name: "Build SCNR Config",
            description: "Builds a detailed SCNR (green-removal) config",
        },
    );
}

fn add_builder<T: Introspect + Clone + fmt::Debug + Send + Sync + 'static>(
    library: &mut Library,
    spec: NodeSpec,
) {
    let func = ConfigValue::<T>::builder(library, spec).category("Astro");
    library.add(func);
}
