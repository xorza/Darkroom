//! Assembly for the `lumos`-backed Astro node library.

mod calibration;
mod io;
mod ml;
mod processing;
mod runtime;
mod stacking;

use std::path::PathBuf;

use scenarium::{Library, TypeEntry};

use crate::astro::config;
use crate::astro::masters::MASTERS_TYPE_ID;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlModelPaths {
    pub denoise: PathBuf,
    pub star_removal: PathBuf,
}

impl Default for MlModelPaths {
    fn default() -> Self {
        Self {
            denoise: PathBuf::from("DeepSNR_weights_v2.onnx"),
            star_removal: PathBuf::from("StarNet2_weights.onnx"),
        }
    }
}

pub fn astro_library(model_paths: &MlModelPaths) -> Library {
    let mut library = Library::default();
    library.register_type(*MASTERS_TYPE_ID, TypeEntry::custom("Masters"));
    config::register_builders(&mut library);
    io::register(&mut library);
    calibration::register(&mut library);
    stacking::register(&mut library);
    processing::register(&mut library);
    ml::register(&mut library, model_paths);
    library
}

pub fn configure_ml_model_defaults(library: &mut Library, model_paths: &MlModelPaths) {
    ml::replace(library, model_paths);
}

#[cfg(test)]
mod tests;
