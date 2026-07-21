//! Astro path types, image loading, and camera-RAW directory scanning.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use anyhow::Context;
use common::file_utils;
use imaginarium::Image as RawImage;
use lumos::{LoadContext, PREVIEW_IMAGE_EXTENSIONS, PreviewImage, RAW_EXTENSIONS};
use scenarium::{DataType, DynamicValue, FsPathConfig, FsPathMode};
use scenarium::{Func, FuncInput, FuncLambda, FuncOutput, Library};

use crate::astro::nodes::runtime;
use crate::image::{IMAGE_DATA_TYPE, Image};

pub(crate) static ASTRO_IMAGE_PATH_DATA_TYPE: LazyLock<DataType> = LazyLock::new(|| {
    DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
        FsPathMode::ExistingFile,
        PREVIEW_IMAGE_EXTENSIONS
            .iter()
            .map(|extension| extension.to_string())
            .collect(),
    )))
});

pub(crate) static ASTRO_DIR_DATA_TYPE: LazyLock<DataType> =
    LazyLock::new(|| DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::Directory))));

pub(crate) fn register(library: &mut Library) {
    library.add(
        Func::new("fbcc8899-efc3-40e0-a6fd-8743f86edbd3", "Load Astro Image")
            .description("Loads a FITS/RAW/standard astronomical image.")
            .category("Astro")
            .pure()
            .input(
                FuncInput::required("Path", ASTRO_IMAGE_PATH_DATA_TYPE.clone())
                    .description("FITS, camera-RAW, or standard image file to load."),
            )
            .output(FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Decoded frame."))
            .lambda(FuncLambda::new(move |ctx, _, _, inputs, _, outputs| {
                let cancel = ctx.cancel_flag();
                Box::pin(async move {
                    debug_assert_eq!(inputs.len(), 1);
                    debug_assert_eq!(outputs.len(), 1);

                    let path = inputs[0]
                        .value
                        .as_fs_path()
                        .expect("path input type is validated at the compile boundary")
                        .to_owned();
                    let image = runtime::run_cancellable(cancel, move |cancel| {
                        let context = LoadContext {
                            cancel,
                            ..Default::default()
                        };
                        PreviewImage::from_file(&path, &context)
                            .map(RawImage::from)
                            .map_err(anyhow::Error::from)
                    })
                    .await?;

                    outputs[0] = DynamicValue::from_custom(Image::from(image));
                    Ok(())
                })
            })),
    );
}

pub(crate) fn raw_frame_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    file_utils::files_with_extensions(dir, RAW_EXTENSIONS)
        .with_context(|| format!("failed to scan camera-RAW frame folder '{}'", dir.display()))
}
