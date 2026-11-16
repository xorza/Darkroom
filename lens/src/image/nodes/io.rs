//! Standard image load and save nodes.

use scenarium::FuncId;
use scenarium::async_lambda;
use std::io::{BufReader, BufWriter, Write};
use std::mem;
use std::path::{Path, PathBuf};

use common::{SystemMemory, fs, task};
use imaginarium::{RasterFormat, SUPPORTED_EXTENSIONS};
use scenarium::{ConstValue, DataType, DynamicValue, FileKind, FileKindId, FileMode, InvokeError};
use scenarium::{Func, InputDecl, Library, OutputDecl};

use crate::image::format::{AS_IS, CONVERSION_FORMAT_DATATYPE, ConversionFormat};
use crate::image::nodes::error::RasterFileError;
use crate::image::{IMAGE_DATA_TYPE, Image};
use scenarium::Invocation;

const LOAD_IMAGE_FUNC_ID: FuncId = FuncId::literal("a4d9bf87-9d98-44f1-a162-7483c298be3d");
const SAVE_IMAGE_FUNC_ID: FuncId = FuncId::literal("0c17bcbe-d757-43be-b184-27b429e8b434");

/// The raster images imaginarium reads and writes.
const IMAGE_FILE: FileKindId = FileKindId::literal("404e04fb-f5c4-43dd-9ee0-684ab9b16200");

pub(super) fn register(library: &mut Library) {
    library.register_file_kind(IMAGE_FILE, FileKind::new("Image", SUPPORTED_EXTENSIONS));
    register_load(library);
    register_save(library);
}

/// The image in the file at `path`, in the format its extension names, refused when its header
/// states more samples than the process can allocate now.
fn read(path: &Path) -> Result<imaginarium::InterleavedImage, RasterFileError> {
    let format = raster_format(path)?;
    Ok(format.decode(BufReader::new(fs::open(path)?), SystemMemory::available())?)
}

/// `image` written to `path` in the format its extension names, published durably: a reader sees
/// the old file or the new one.
fn save(image: &imaginarium::InterleavedImage, path: &Path) -> Result<(), RasterFileError> {
    let format = raster_format(path)?;
    fs::publish(path, fs::PublicationMode::Durable, |file| {
        let mut out = BufWriter::new(file);
        format.encode(image, &mut out)?;
        Ok::<_, RasterFileError>(out.flush()?)
    })
}

/// The format the extension of `path` names.
fn raster_format(path: &Path) -> Result<RasterFormat, RasterFileError> {
    RasterFormat::from_path(path).ok_or_else(|| RasterFileError::Extension {
        path: path.to_path_buf(),
    })
}

fn register_load(library: &mut Library) {
    library.add(
        Func::builder(
            LOAD_IMAGE_FUNC_ID,
            "Load Image",
            async_lambda!(move |Invocation {
                                    inputs, outputs, ..
                                }| {
                debug_assert_eq!(inputs.len(), 1);
                debug_assert_eq!(outputs.len(), 1);
                let path = PathBuf::from(inputs[0].required_fs_path());
                let image = task::run_blocking(move || read(&path))
                    .await
                    .map_err(|task::Shutdown| InvokeError::Cancelled)?
                    .map_err(InvokeError::external)?;
                outputs[0] = DynamicValue::from_custom(Image::from(image));
                Ok(())
            }),
        )
        .description("Loads an image from a file on disk.")
        .category("Image")
        .pure()
        .input(
            InputDecl::required("Path", image_file(FileMode::Open))
                .description("Image file to load."),
        )
        .output(OutputDecl::new("Image", IMAGE_DATA_TYPE).description("Loaded image."))
        .build(),
    );
}

fn register_save(library: &mut Library) {
    library.add(
        Func::builder(SAVE_IMAGE_FUNC_ID, "Save Image", async_lambda!(move |Invocation { inputs, .. }| {
                    debug_assert_eq!(inputs.len(), 3);
                    let value = mem::take(&mut inputs[0]);
                    let path = PathBuf::from(
                        inputs[1]
                            .required_fs_path(),
                    );
                    let format = inputs[2].required_enum_as::<ConversionFormat>();
                    // Saving reads the pixels, so the value moves to the save and is read where it
                    // lies: an interleaved image is not copied, a planar one is interleaved once.
                    task::run_blocking(move || {
                        let image = value.required_custom::<Image>();
                        let pixels = image.interleaved();
                        match format.target(pixels.desc().color_format()) {
                            Some(target) => save(&pixels.convert_to(target), &path),
                            None => save(&pixels, &path),
                        }
                    })
                    .await
                    .map_err(|task::Shutdown| InvokeError::Cancelled)?
                    .map_err(InvokeError::external)?;
                    Ok(())
                }))
            .description("Writes an image to a file on disk.")
            .category("Image")
            .sink()
            .input(
                InputDecl::required("Image", IMAGE_DATA_TYPE).description("Image to save."),
            )
            .input(
                InputDecl::required("Path", image_file(FileMode::Save))
                    .description("Destination file; the extension picks the container."),
            )
            .input(
                InputDecl::required("Format", CONVERSION_FORMAT_DATATYPE)
                    .default(ConstValue::Enum(AS_IS.to_string()))
                    .description(
                        "Convert to this color format before saving; \"As Is\" keeps the source format.",
                    ),
            )
            .build(),
    );
}

const fn image_file(mode: FileMode) -> DataType {
    DataType::File {
        kind: IMAGE_FILE,
        mode,
    }
}
