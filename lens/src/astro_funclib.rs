//! `AstroFuncLib` — the `lumos`-backed node library (category `astro`).
//!
//! Phase 1 ships one proof node, `load_astro_image`, that decodes a
//! FITS/RAW/standard file into an [`AstroFrame`]. The heavier masters /
//! stacking / processing nodes build on these types in later phases.

use std::sync::{Arc, LazyLock};

use lumos::AstroImage;
use scenarium::data::{DataType, DynamicValue, FsPathConfig, FsPathMode};
use scenarium::func_lambda::FuncLambda;
use scenarium::function::{Func, FuncBehavior, FuncInput, FuncLib, FuncOutput};
use scenarium::graph::NodeBehavior;

use crate::astro_frame::{ASTRO_FRAME_DATA_TYPE, AstroFrame};

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

        Self { func_lib }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn astro_image_path_filter_matches_from_file_extensions() {
        let DataType::FsPath(cfg) = &*ASTRO_IMAGE_PATH_DATA_TYPE else {
            panic!("expected an FsPath data type");
        };
        assert_eq!(cfg.mode, FsPathMode::ExistingFile);
        assert_eq!(cfg.extensions, ASTRO_IMAGE_EXTENSIONS);
    }

    #[test]
    fn load_astro_image_node_is_registered() {
        let lib = AstroFuncLib::default();
        let func = lib
            .func_lib()
            .funcs
            .iter()
            .find(|f| f.name == "load_astro_image")
            .expect("load_astro_image registered");
        assert_eq!(func.category, "astro");
        assert_eq!(func.inputs.len(), 1);
        assert_eq!(func.outputs.len(), 1);
        assert_eq!(func.inputs[0].data_type, *ASTRO_IMAGE_PATH_DATA_TYPE);
        assert_eq!(func.outputs[0].data_type, *ASTRO_FRAME_DATA_TYPE);
    }
}
