//! Standard image load and save nodes.

use std::path::PathBuf;
use std::sync::Arc;

use imaginarium::SUPPORTED_EXTENSIONS;
use scenarium::{DataType, DynamicValue, FsPathConfig, FsPathMode, InvokeError, StaticValue};
use scenarium::{Func, FuncInput, FuncLambda, FuncOutput, Library};

use crate::config_node::enum_input;
use crate::image::context::{VISION_CTX_TYPE, VisionCtx};
use crate::image::format::{CONVERSION_FORMAT_DATATYPE, ConversionFormat, conversion_target};
use crate::image::{IMAGE_DATA_TYPE, Image};

pub(crate) fn register(library: &mut Library) {
    register_load(library);
    register_save(library);
}

fn register_load(library: &mut Library) {
    library.add(
        Func::new("a4d9bf87-9d98-44f1-a162-7483c298be3d", "Load Image")
            .description("Loads an image from a file on disk.")
            .category("Image")
            .pure()
            .input(
                FuncInput::required("Path", image_fs_path(FsPathMode::ExistingFile))
                    .description("Image file to load."),
            )
            .output(FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Loaded image."))
            .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
                Box::pin(async move {
                    debug_assert_eq!(inputs.len(), 1);
                    debug_assert_eq!(outputs.len(), 1);
                    let path = PathBuf::from(
                        inputs[0]
                            .value
                            .as_fs_path()
                            .expect("path input type is validated at the compile boundary"),
                    );
                    let image = tokio::task::spawn_blocking(move || {
                        imaginarium::Image::read_file(path).map_err(InvokeError::external)
                    })
                    .await
                    .map_err(InvokeError::external)??;
                    outputs[0] = DynamicValue::from_custom(Image::from(image));
                    Ok(())
                })
            })),
    );
}

fn register_save(library: &mut Library) {
    library.add(
        Func::new("0c17bcbe-d757-43be-b184-27b429e8b434", "Save Image")
            .description("Writes an image to a file on disk.")
            .category("Image")
            .sink()
            .input(
                FuncInput::required("Image", IMAGE_DATA_TYPE.clone()).description("Image to save."),
            )
            .input(
                FuncInput::required("Path", image_fs_path(FsPathMode::NewFile))
                    .description("Destination file; the extension picks the container."),
            )
            .input(
                enum_input::<ConversionFormat>("Format", &CONVERSION_FORMAT_DATATYPE)
                    .default(StaticValue::Enum(ConversionFormat::AsIs.label()))
                    .description(
                        "Convert to this color format before saving; \"As Is\" keeps the source format.",
                    ),
            )
            .context(VISION_CTX_TYPE.clone())
            .lambda(FuncLambda::new(move |contexts, _, _, inputs, _, _| {
                Box::pin(async move {
                    debug_assert_eq!(inputs.len(), 3);
                    let value = std::mem::take(&mut inputs[0].value);
                    let path = PathBuf::from(
                        inputs[1]
                            .value
                            .as_fs_path()
                            .expect("path input type is validated at the compile boundary"),
                    );
                    let format = inputs[2]
                        .value
                        .as_enum()
                        .expect("format input type is validated at the compile boundary")
                        .to_owned();
                    let cpu_image = {
                        let vision = contexts.get::<VisionCtx>(&VISION_CTX_TYPE);
                        match value.into_custom::<Image>() {
                            Ok(image) => image
                                .buffer
                                .to_cpu(&vision.processing_ctx)
                                .map_err(InvokeError::external)?,
                            Err(value) => value
                                .as_custom::<Image>()
                                .expect("image input type is validated at the compile boundary")
                                .buffer
                                .make_cpu(&vision.processing_ctx)
                                .map_err(InvokeError::external)?
                                .clone(),
                        }
                    };
                    tokio::task::spawn_blocking(move || {
                        match conversion_target(&format, cpu_image.desc().color_format) {
                            Some(target) => cpu_image
                                .convert_to(target)
                                .map_err(InvokeError::external)?
                                .save_file(path),
                            None => cpu_image.save_file(path),
                        }
                        .map_err(InvokeError::external)
                    })
                    .await
                    .map_err(InvokeError::external)??;
                    Ok(())
                })
            })),
    );
}

fn image_fs_path(mode: FsPathMode) -> DataType {
    DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
        mode,
        SUPPORTED_EXTENSIONS
            .iter()
            .map(|extension| extension.to_string())
            .collect(),
    )))
}
