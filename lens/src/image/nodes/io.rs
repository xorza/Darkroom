//! Standard image load and save nodes.

use scenarium::FuncId;
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;

use imaginarium::SUPPORTED_EXTENSIONS;
use scenarium::{ConstValue, DataType, DynamicValue, FsPathConfig, FsPathMode, InvokeError};
use scenarium::{Func, FuncInput, FuncLambda, FuncOutput, Library};

use crate::config_node::enum_input;
use crate::image::format::{
    AS_IS, CONVERSION_FORMAT_DATATYPE, ConversionFormat, conversion_target,
};
use crate::image::{IMAGE_DATA_TYPE, Image};
use scenarium::Invocation;
use tokio::task;

const LOAD_IMAGE_FUNC_ID: FuncId = FuncId::literal("a4d9bf87-9d98-44f1-a162-7483c298be3d");
const SAVE_IMAGE_FUNC_ID: FuncId = FuncId::literal("0c17bcbe-d757-43be-b184-27b429e8b434");

pub(super) fn register(library: &mut Library) {
    register_load(library);
    register_save(library);
}

fn register_load(library: &mut Library) {
    library.add(
        Func::new(
            LOAD_IMAGE_FUNC_ID,
            "Load Image",
            FuncLambda::new(
                move |Invocation {
                          inputs, outputs, ..
                      }| {
                    Box::pin(async move {
                        debug_assert_eq!(inputs.len(), 1);
                        debug_assert_eq!(outputs.len(), 1);
                        let path = PathBuf::from(inputs[0].required_fs_path());
                        let image = task::spawn_blocking(move || {
                            imaginarium::Image::read_file(path).map_err(InvokeError::external)
                        })
                        .await
                        .map_err(InvokeError::external)??;
                        outputs[0] = DynamicValue::from_custom(Image::from(image));
                        Ok(())
                    })
                },
            ),
        )
        .description("Loads an image from a file on disk.")
        .category("Image")
        .pure()
        .input(
            FuncInput::required("Path", image_fs_path(FsPathMode::ExistingFile))
                .description("Image file to load."),
        )
        .output(FuncOutput::new("Image", IMAGE_DATA_TYPE.clone()).description("Loaded image.")),
    );
}

fn register_save(library: &mut Library) {
    library.add(
        Func::new(SAVE_IMAGE_FUNC_ID, "Save Image", FuncLambda::new(move |Invocation { inputs, .. }| {
                Box::pin(async move {
                    debug_assert_eq!(inputs.len(), 3);
                    let value = mem::take(&mut inputs[0]);
                    let path = PathBuf::from(
                        inputs[1]
                            .required_fs_path(),
                    );
                    let format = inputs[2]
                        .required_enum()
                        .to_owned();
                    // Saving needs the pixels by value anyway, so take them when this is the last
                    // holder and copy only when it is not.
                    let cpu_image = Image::take_interleaved(value);
                    task::spawn_blocking(move || {
                        match conversion_target(&format, cpu_image.desc().color_format) {
                            Some(target) => cpu_image.convert_to(target).save_file(path),
                            None => cpu_image.save_file(path),
                        }
                        .map_err(InvokeError::external)
                    })
                    .await
                    .map_err(InvokeError::external)??;
                    Ok(())
                })
            }))
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
                    .default(ConstValue::Enum(AS_IS.to_string()))
                    .description(
                        "Convert to this color format before saving; \"As Is\" keeps the source format.",
                    ),
            ),
    );
}

fn image_fs_path(mode: FsPathMode) -> DataType {
    DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
        mode,
        SUPPORTED_EXTENSIONS
            .iter()
            .map(ToString::to_string)
            .collect(),
    )))
}
