//! Astro path types and image loading.

use lumos::{DecodeConfig, LoadCtx, PREVIEW_IMAGE_EXTENSIONS, RAW_EXTENSIONS};
use scenarium::{
    DataType, DynamicValue, FileKind, FileKindId, FileMode, Func, FuncId, InputDecl, Invocation,
    LibraryBuilder, OutputDecl, async_lambda,
};

use crate::astro::nodes::runtime;
use crate::image::{IMAGE_DATA_TYPE, Image};

const LOAD_ASTRO_IMAGE_FUNC_ID: FuncId = FuncId::literal("fbcc8899-efc3-40e0-a6fd-8743f86edbd3");

/// The files a preview opens: FITS, camera RAW and every raster format.
const ASTRO_IMAGE_FILE: FileKindId = FileKindId::literal("5009fdc9-bc05-4fe1-acaf-8e13bce260bf");
/// Camera RAW files.
const RAW_FILE: FileKindId = FileKindId::literal("48ace2a5-0d6c-423c-a67c-48108930fd37");

/// One image file of a format a preview opens.
pub(super) const fn astro_image_path_type() -> DataType {
    DataType::File {
        kind: ASTRO_IMAGE_FILE,
        mode: FileMode::Open,
    }
}

/// Raw files, one or more.
pub(super) const fn astro_raw_paths_type() -> DataType {
    DataType::File {
        kind: RAW_FILE,
        mode: FileMode::OpenMany,
    }
}

pub(crate) fn register(library: &mut LibraryBuilder) {
    library.add_file_kind(
        ASTRO_IMAGE_FILE,
        FileKind::new("Astro Image", PREVIEW_IMAGE_EXTENSIONS.iter().copied()),
    );
    library.add_file_kind(
        RAW_FILE,
        FileKind::new("Camera RAW", RAW_EXTENSIONS.iter().copied()),
    );
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
                    let ctx = LoadCtx::new(DecodeConfig::default(), cancel);
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
            1,
            InputDecl::required("Path", astro_image_path_type())
                .description("FITS, camera-RAW, or standard image file to load."),
        )
        .output(
            1,
            OutputDecl::new("Image", IMAGE_DATA_TYPE).description("Decoded frame."),
        )
        .build(),
    );
}
