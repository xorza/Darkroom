//! Astro path types and image loading.

use std::sync::Arc;

use lumos::{LoadCtx, PREVIEW_IMAGE_EXTENSIONS, RAW_EXTENSIONS};
use scenarium::{
    DataType, DynamicValue, FsPathConfig, FsPathMode, Func, FuncId, FuncInput, FuncOutput,
    Invocation, Library, async_lambda,
};

use crate::astro::nodes::runtime;
use crate::image::{IMAGE_DATA_TYPE, Image};

const LOAD_ASTRO_IMAGE_FUNC_ID: FuncId = FuncId::literal("fbcc8899-efc3-40e0-a6fd-8743f86edbd3");

/// One image file of a format a preview opens.
pub(super) fn astro_image_path_type() -> DataType {
    DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
        FsPathMode::ExistingFile,
        PREVIEW_IMAGE_EXTENSIONS
            .iter()
            .map(ToString::to_string)
            .collect(),
    )))
}

/// Raw files, one or more.
pub(super) fn astro_raw_paths_type() -> DataType {
    DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
        FsPathMode::ExistingFiles,
        RAW_EXTENSIONS.iter().map(ToString::to_string).collect(),
    )))
}

pub(crate) fn register(library: &mut Library) {
    library.add(
        Func::builder(
            LOAD_ASTRO_IMAGE_FUNC_ID,
            "Load Astro Image",
            async_lambda!(move |Invocation { ctx, inputs, outputs, .. }| {
                cancel = ctx.cancel_flag(),
            } => {
                debug_assert_eq!(inputs.len(), 1);
                debug_assert_eq!(outputs.len(), 1);

                let path = inputs[0].required_fs_path().to_owned();
                let image = runtime::run_cancellable(cancel, move |cancel| {
                    let ctx = LoadCtx {
                        cancel,
                        ..Default::default()
                    };
                    ctx.preview(&path)
                })
                .await?;

                outputs[0] = DynamicValue::from_custom(Image::from(image));
                Ok(())
            }),
        )
        .description("Loads a FITS/RAW/standard astronomical image.")
        .category("Astro")
        .pure()
        .input(
            FuncInput::required("Path", astro_image_path_type())
                .description("FITS, camera-RAW, or standard image file to load."),
        )
        .output(FuncOutput::new("Image", IMAGE_DATA_TYPE).description("Decoded frame."))
        .build(),
    );
}
