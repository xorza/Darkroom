//! Assembly for the `lumos`-backed Astro node library.

mod calibration;
mod io;
mod ml;
mod processing;
mod runtime;
mod stacking;

use std::path::PathBuf;

use scenarium::{CustomType, Library};

use crate::astro::config;
use crate::astro::masters::MASTERS_TYPE_ID;

/// The ONNX files the ML nodes load, each unset until a person picks one: lumos ships no model,
/// and a bare file name would resolve against a working directory nobody chose. Serializable so a
/// frontend can persist it as it stands; a missing field reads as unset.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MlModelPaths {
    pub denoise: Option<PathBuf>,
    pub star_removal: Option<PathBuf>,
}

pub fn astro_library(model_paths: &MlModelPaths) -> Library {
    let mut library = Library::default();
    library.register_custom_type(MASTERS_TYPE_ID, CustomType::new("Masters"));
    config::register_builders(&mut library);
    io::register(&mut library);
    calibration::register(&mut library);
    stacking::register(&mut library);
    processing::register(&mut library);
    ml::register(&mut library, model_paths);
    library
}

#[cfg(test)]
mod tests;
