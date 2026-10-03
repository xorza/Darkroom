//! Astro path types and image loading.

use scenarium::FuncId;
use scenarium::async_lambda;
use std::sync::{Arc, LazyLock};

use lumos::{LoadContext, PREVIEW_IMAGE_EXTENSIONS, PreviewImage, PreviewPixels, RAW_EXTENSIONS};
use scenarium::{DataType, DynamicValue, FsPathConfig, FsPathMode};
use scenarium::{Func, FuncInput, FuncOutput, Library};

use crate::astro::nodes::runtime;
use crate::image::{IMAGE_DATA_TYPE, Image};
use scenarium::Invocation;

const LOAD_ASTRO_IMAGE_FUNC_ID: FuncId = FuncId::literal("fbcc8899-efc3-40e0-a6fd-8743f86edbd3");

pub(super) static ASTRO_IMAGE_PATH_DATA_TYPE: LazyLock<DataType> = LazyLock::new(|| {
    DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
        FsPathMode::ExistingFile,
        PREVIEW_IMAGE_EXTENSIONS
            .iter()
            .map(ToString::to_string)
            .collect(),
    )))
});

pub(super) static ASTRO_RAW_PATHS_DATA_TYPE: LazyLock<DataType> = LazyLock::new(|| {
    DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
        FsPathMode::ExistingFiles,
        RAW_EXTENSIONS.iter().map(ToString::to_string).collect(),
    )))
});

pub(crate) fn register(library: &mut Library) {
    library.add(
        Func::new(
            LOAD_ASTRO_IMAGE_FUNC_ID,
            "Load Astro Image",
            async_lambda!(move |Invocation { ctx, inputs, outputs, .. }| {
                cancel = ctx.cancel_flag(),
            } => {
                debug_assert_eq!(inputs.len(), 1);
                debug_assert_eq!(outputs.len(), 1);

                let path = inputs[0].required_fs_path().to_owned();
                let image = runtime::run_cancellable(cancel, move |cancel| {
                    let context = LoadContext {
                        cancel,
                        ..Default::default()
                    };
                    PreviewImage::from_file(&path, &context)
                })
                .await?;

                outputs[0] = DynamicValue::from_custom(match image.into_pixels() {
                    PreviewPixels::Planes(planes) => Image::from(planes),
                    PreviewPixels::Interleaved(samples) => Image::from(samples),
                });
                Ok(())
            }),
        )
        .description("Loads a FITS/RAW/standard astronomical image.")
        .category("Astro")
        .pure()
        .input(
            FuncInput::required("Path", ASTRO_IMAGE_PATH_DATA_TYPE.clone())
                .description("FITS, camera-RAW, or standard image file to load."),
        )
        .output(FuncOutput::new("Image", IMAGE_DATA_TYPE).description("Decoded frame.")),
    );
}
