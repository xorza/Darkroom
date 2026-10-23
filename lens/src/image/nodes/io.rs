//! Standard image load and save nodes.

use scenarium::FuncId;
use scenarium::async_lambda;
use std::io::{BufReader, BufWriter, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{FixedFile, SystemMemory, fs, task};
use imaginarium::{AdmittedDecodeError, FileFormat, SUPPORTED_EXTENSIONS};
use scenarium::{ConstValue, DataType, DynamicValue, FsPathConfig, FsPathMode, InvokeError};
use scenarium::{Func, FuncInput, FuncOutput, Library};

use crate::image::format::{AS_IS, CONVERSION_FORMAT_DATATYPE, ConversionFormat};
use crate::image::{IMAGE_DATA_TYPE, Image};
use scenarium::Invocation;

const LOAD_IMAGE_FUNC_ID: FuncId = FuncId::literal("a4d9bf87-9d98-44f1-a162-7483c298be3d");
const SAVE_IMAGE_FUNC_ID: FuncId = FuncId::literal("0c17bcbe-d757-43be-b184-27b429e8b434");

pub(super) fn register(library: &mut Library) {
    register_load(library);
    register_save(library);
}

/// Why an image file did not load.
#[derive(Debug, thiserror::Error)]
enum ReadError {
    #[error(transparent)]
    Image(#[from] imaginarium::Error),
    #[error("the decode needs {required} bytes, past the {available} this process can allocate")]
    OverMemory { required: u64, available: u64 },
}

/// The image in the file at `path`, in the format its extension names, refused from its header
/// when it needs more than this process can allocate: the image, beside the file when the format
/// reads its whole input, which is refused before it is read.
fn read(path: &Path) -> Result<imaginarium::Image, ReadError> {
    let format = FileFormat::from_path(path)?;
    let file = FixedFile::open(path).map_err(imaginarium::Error::from)?;
    let held = if format.reads_whole_input() {
        file.len()
    } else {
        0
    };
    let available = SystemMemory::available();
    let admit = |required: u64| {
        if required > available {
            return Err(ReadError::OverMemory {
                required,
                available,
            });
        }
        Ok(())
    };
    admit(held)?;
    imaginarium::Image::decode_admitted(BufReader::new(file), format, |desc| {
        admit(held + u64::try_from(desc.size_in_bytes()).expect("a byte count fits u64"))
    })
    .map_err(|error| match error {
        AdmittedDecodeError::Refused(error) => error,
        AdmittedDecodeError::Decode(error) => ReadError::Image(error),
    })
}

/// `image` written to `path` in the format its extension names, published durably: a reader sees
/// the old file or the new one.
fn save(image: &imaginarium::Image, path: &Path) -> Result<(), imaginarium::Error> {
    let format = FileFormat::from_path(path)?;
    fs::publish(path, fs::PublicationMode::Durable, |file| {
        let mut out = BufWriter::new(file);
        image.encode(&mut out, format)?;
        Ok::<_, imaginarium::Error>(out.flush()?)
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
                let image =
                    task::spawn_blocking(move || read(&path).map_err(InvokeError::external))
                        .await
                        .map_err(InvokeError::external)??;
                outputs[0] = DynamicValue::from_custom(Image::from(image));
                Ok(())
            }),
        )
        .description("Loads an image from a file on disk.")
        .category("Image")
        .pure()
        .input(
            FuncInput::required("Path", image_fs_path(FsPathMode::ExistingFile))
                .description("Image file to load."),
        )
        .output(FuncOutput::new("Image", IMAGE_DATA_TYPE).description("Loaded image."))
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
                    // Saving needs the pixels by value anyway, so take them when this is the last
                    // holder and copy only when it is not.
                    let cpu_image = Image::take_interleaved(value);
                    task::spawn_blocking(move || {
                        match format.target(cpu_image.desc().color_format) {
                            Some(target) => save(&cpu_image.convert_to(target), &path),
                            None => save(&cpu_image, &path),
                        }
                        .map_err(InvokeError::external)
                    })
                    .await
                    .map_err(InvokeError::external)??;
                    Ok(())
                }))
            .description("Writes an image to a file on disk.")
            .category("Image")
            .sink()
            .input(
                FuncInput::required("Image", IMAGE_DATA_TYPE).description("Image to save."),
            )
            .input(
                FuncInput::required("Path", image_fs_path(FsPathMode::NewFile))
                    .description("Destination file; the extension picks the container."),
            )
            .input(
                FuncInput::required("Format", CONVERSION_FORMAT_DATATYPE)
                    .default(ConstValue::Enum(AS_IS.to_string()))
                    .description(
                        "Convert to this color format before saving; \"As Is\" keeps the source format.",
                    ),
            )
            .build(),
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

#[cfg(test)]
mod tests {
    use common::{TempDir, fs};

    use crate::image::nodes::io::{ReadError, read};

    /// A TIFF whose header declares 1 000 000 × 1 000 000 grey f32 pixels, 4 TB, over one strip of
    /// four bytes, its tables consistent, is refused from its header for those bytes.
    #[test]
    fn a_load_past_memory_is_refused_from_its_header() {
        const SIDE: u32 = 1_000_000;
        // Tag, type (3 SHORT, 4 LONG), value: one value each, held in its entry.
        let entries: [(u16, u16, u32); 10] = [
            (256, 4, SIDE),
            (257, 4, SIDE),
            (258, 3, 32),
            (259, 3, 1),
            (262, 3, 1),
            (273, 4, 8),
            (277, 3, 1),
            (278, 4, SIDE),
            (279, 4, 4),
            (339, 3, 3),
        ];
        let mut tiff = b"II*\0".to_vec();
        tiff.extend(12u32.to_le_bytes());
        tiff.extend(0.5f32.to_le_bytes());
        tiff.extend(10u16.to_le_bytes());
        for (tag, kind, value) in entries {
            tiff.extend(tag.to_le_bytes());
            tiff.extend(kind.to_le_bytes());
            tiff.extend(1u32.to_le_bytes());
            tiff.extend(value.to_le_bytes());
        }
        tiff.extend(0u32.to_le_bytes());

        let dir = TempDir::new("lens-image-io");
        let path = dir.join("crafted.tiff");
        fs::write(&path, &tiff).unwrap();
        let refused = read(&path);
        assert!(
            matches!(
                refused,
                Err(ReadError::OverMemory {
                    required: 4_000_000_000_000,
                    ..
                })
            ),
            "{refused:?}"
        );
    }
}
