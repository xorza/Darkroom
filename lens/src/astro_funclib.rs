//! `AstroFuncLib` — the `lumos`-backed node library (category `astro`).
//!
//! Phase 1 ships one proof node, `load_astro_image`, that decodes a
//! FITS/RAW/standard file into an [`AstroFrame`]. The heavier masters /
//! stacking / processing nodes build on these types in later phases.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use common::file_utils::astro_image_files;
use lumos::{AstroImage, CalibrationFrames, CalibrationMasters, DEFAULT_SIGMA_THRESHOLD};
use scenarium::data::{DataType, DynamicValue, FsPathConfig, FsPathMode};
use scenarium::func_lambda::FuncLambda;
use scenarium::function::{Func, FuncBehavior, FuncInput, FuncLib, FuncOutput};
use scenarium::graph::NodeBehavior;

use crate::astro_frame::{ASTRO_FRAME_DATA_TYPE, AstroFrame};
use crate::masters::{MASTERS_DATA_TYPE, Masters};

/// Every file extension `AstroImage::from_file` recognizes: FITS, camera
/// RAW, and standard images. Kept in lockstep with that dispatch (note
/// `fts` is *not* accepted, so it's absent here).
const ASTRO_IMAGE_EXTENSIONS: [&str; 13] = [
    "fits", "fit", // FITS
    "raf", "cr2", "cr3", "nef", "arw", "dng", // camera RAW
    "tiff", "tif", "png", "jpg", "jpeg", // standard
];

/// Reusable data type for an astro image file-path input: an existing-file
/// picker filtered to [`ASTRO_IMAGE_EXTENSIONS`]. Shared so every astro
/// loader presents the same filter.
pub static ASTRO_IMAGE_PATH_DATA_TYPE: LazyLock<DataType> = LazyLock::new(|| {
    DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
        FsPathMode::ExistingFile,
        ASTRO_IMAGE_EXTENSIONS
            .iter()
            .map(|s| s.to_string())
            .collect(),
    )))
});

/// Reusable data type for a frame-folder input: an existing-directory
/// picker (no extension filter). Shared by the masters / stacking nodes,
/// which glob the directory for astro frames via
/// [`common::file_utils::astro_image_files`].
pub static ASTRO_DIR_DATA_TYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::Directory))));

#[derive(Debug)]
pub struct AstroFuncLib {
    func_lib: FuncLib,
}

impl AstroFuncLib {
    pub fn func_lib(&self) -> &FuncLib {
        &self.func_lib
    }

    pub fn into_func_lib(self) -> FuncLib {
        self.func_lib
    }
}

impl From<AstroFuncLib> for FuncLib {
    fn from(astro: AstroFuncLib) -> Self {
        astro.func_lib
    }
}

impl Default for AstroFuncLib {
    fn default() -> Self {
        let mut func_lib = FuncLib::default();

        // load_astro_image
        func_lib.add(Func {
            id: "f1a2b3c4-d5e6-4f70-8a91-b2c3d4e5f610".into(),
            name: "load_astro_image".to_string(),
            description: Some("Loads a FITS/RAW/standard astronomical image".to_string()),
            behavior: FuncBehavior::Impure,
            node_default_behavior: NodeBehavior::Once,
            terminal: false,
            category: "astro".to_string(),
            inputs: vec![FuncInput {
                name: "path".to_string(),
                required: true,
                data_type: ASTRO_IMAGE_PATH_DATA_TYPE.clone(),
                default_value: None,
                value_options: vec![],
            }],
            outputs: vec![FuncOutput {
                name: "image".to_string(),
                data_type: ASTRO_FRAME_DATA_TYPE.clone(),
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 1);
                    assert_eq!(outputs.len(), 1);

                    let path = inputs[0].value.as_fs_path().unwrap().to_owned();
                    // Decoding (FITS parse / libraw / demosaic) is heavy
                    // synchronous CPU work — keep it off the worker thread.
                    let image = tokio::task::spawn_blocking(move || AstroImage::from_file(&path))
                        .await
                        .map_err(anyhow::Error::from)?
                        .map_err(anyhow::Error::from)?;

                    outputs[0] = DynamicValue::from_custom(AstroFrame::from(image));

                    Ok(())
                })
            }),
        });

        // build_masters
        func_lib.add(Func {
            id: "f1a2b3c4-d5e6-4f70-8a91-b2c3d4e5f611".into(),
            name: "build_masters".to_string(),
            description: Some(
                "Stacks raw calibration frames (darks/flats/bias/flat-darks) into calibration \
                 masters"
                    .to_string(),
            ),
            behavior: FuncBehavior::Impure,
            node_default_behavior: NodeBehavior::Once,
            terminal: false,
            category: "astro".to_string(),
            inputs: vec![
                dir_input("darks"),
                dir_input("flats"),
                dir_input("bias"),
                dir_input("flat_darks"),
                FuncInput {
                    name: "sigma".to_string(),
                    required: false,
                    data_type: DataType::Float,
                    default_value: Some((DEFAULT_SIGMA_THRESHOLD as f64).into()),
                    value_options: vec![],
                },
            ],
            outputs: vec![FuncOutput {
                name: "masters".to_string(),
                data_type: MASTERS_DATA_TYPE.clone(),
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    assert_eq!(inputs.len(), 5);
                    assert_eq!(outputs.len(), 1);

                    // Each optional folder globs to its astro frames (empty
                    // when the port is unbound or the directory is unreadable).
                    let frames_in = |idx: usize| -> Vec<PathBuf> {
                        inputs[idx]
                            .value
                            .as_fs_path()
                            .map(|dir| astro_image_files(Path::new(dir)))
                            .unwrap_or_default()
                    };
                    let darks = frames_in(0);
                    let flats = frames_in(1);
                    let bias = frames_in(2);
                    let flat_darks = frames_in(3);
                    let sigma = inputs[4]
                        .value
                        .as_f64()
                        .map(|v| v as f32)
                        .unwrap_or(DEFAULT_SIGMA_THRESHOLD);

                    // Stacking many full-resolution CFA frames is heavy CPU work.
                    let masters = tokio::task::spawn_blocking(move || {
                        CalibrationMasters::from_files(
                            CalibrationFrames {
                                darks: &darks,
                                flats: &flats,
                                bias: &bias,
                                flat_darks: &flat_darks,
                            },
                            sigma,
                        )
                    })
                    .await
                    .map_err(anyhow::Error::from)?
                    .map_err(anyhow::Error::from)?;

                    outputs[0] = DynamicValue::from_custom(Masters::from(masters));

                    Ok(())
                })
            }),
        });

        Self { func_lib }
    }
}

/// An optional calibration-frame folder input (`darks`/`flats`/…): an
/// [`ASTRO_DIR_DATA_TYPE`] directory picker, not required (an unwired role
/// simply yields no master for it).
fn dir_input(name: &str) -> FuncInput {
    FuncInput {
        name: name.to_string(),
        required: false,
        data_type: ASTRO_DIR_DATA_TYPE.clone(),
        default_value: None,
        value_options: vec![],
    }
}

#[cfg(test)]
mod tests {
    use scenarium::data::StaticValue;

    use super::*;

    fn func<'a>(lib: &'a AstroFuncLib, name: &str) -> &'a Func {
        lib.func_lib()
            .funcs
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("{name} registered"))
    }

    #[test]
    fn astro_image_path_filter_matches_from_file_extensions() {
        let DataType::FsPath(cfg) = &*ASTRO_IMAGE_PATH_DATA_TYPE else {
            panic!("expected an FsPath data type");
        };
        assert_eq!(cfg.mode, FsPathMode::ExistingFile);
        assert_eq!(cfg.extensions, ASTRO_IMAGE_EXTENSIONS);
    }

    #[test]
    fn astro_dir_is_an_existing_directory_picker() {
        let DataType::FsPath(cfg) = &*ASTRO_DIR_DATA_TYPE else {
            panic!("expected an FsPath data type");
        };
        assert_eq!(cfg.mode, FsPathMode::Directory);
        assert!(cfg.extensions.is_empty());
    }

    #[test]
    fn load_astro_image_node_is_registered() {
        let lib = AstroFuncLib::default();
        let f = func(&lib, "load_astro_image");
        assert_eq!(f.category, "astro");
        assert_eq!(f.inputs.len(), 1);
        assert_eq!(f.outputs.len(), 1);
        assert_eq!(f.inputs[0].data_type, *ASTRO_IMAGE_PATH_DATA_TYPE);
        assert_eq!(f.outputs[0].data_type, *ASTRO_FRAME_DATA_TYPE);
    }

    #[test]
    fn build_masters_node_is_registered() {
        let lib = AstroFuncLib::default();
        let f = func(&lib, "build_masters");
        assert_eq!(f.category, "astro");
        assert_eq!(f.outputs.len(), 1);
        assert_eq!(f.outputs[0].data_type, *MASTERS_DATA_TYPE);

        // Four optional calibration-frame folders, then sigma.
        assert_eq!(f.inputs.len(), 5);
        let dir_names: Vec<&str> = f.inputs[..4].iter().map(|i| i.name.as_str()).collect();
        assert_eq!(dir_names, ["darks", "flats", "bias", "flat_darks"]);
        for input in &f.inputs[..4] {
            assert!(!input.required, "calibration folders are optional");
            assert_eq!(input.data_type, *ASTRO_DIR_DATA_TYPE);
        }
        assert_eq!(f.inputs[4].name, "sigma");
        assert_eq!(f.inputs[4].data_type, DataType::Float);
        assert_eq!(
            f.inputs[4].default_value,
            Some(StaticValue::Float(DEFAULT_SIGMA_THRESHOLD as f64)),
        );
    }
}
