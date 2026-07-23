//! Calibration-master node and source-aware on-disk master cache.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use common::CancelToken;
use common::file_utils::{self, PublicationMode};
use lumos::{
    CalibrationMasters, CalibrationSet, CfaImage, DEFAULT_SIGMA_THRESHOLD, LoadContext,
    StackConfig, stack_cfa_master,
};
use scenarium::{DataType, DynamicValue, Func, FuncInput, FuncLambda, FuncOutput, Library};

use crate::astro::masters::{MASTERS_DATA_TYPE, Masters};
use crate::astro::nodes::io::{self as astro_io, ASTRO_DIR_DATA_TYPE};
use crate::astro::nodes::runtime;

const CACHE_PRESENT: &str = "present:";
const CACHE_ABSENT: &str = "absent:";

#[derive(Debug, thiserror::Error)]
pub(crate) enum FrameSetKeyError {
    #[error("failed to read metadata for '{path}': {source}", path = .path.display())]
    Metadata {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BuildMastersError {
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Scan(#[from] astro_io::RawFrameScanError),
    #[error(transparent)]
    FrameSet(#[from] FrameSetKeyError),
    #[error(transparent)]
    Stack(#[from] lumos::StackError),
    #[error("failed to update calibration cache '{path}': {source}", path = .path.display())]
    Cache {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub(crate) fn register(library: &mut Library) {
    library.add(
        Func::new("f2f6f1ff-5b10-409c-900f-d6b48750a529", "Build Masters")
            .description(
                "Stacks raw calibration frames (darks/flats/bias/flat-darks) into calibration \
                 masters. With `cache` on, each master is written next to its frames and reused \
                 while the source-frame set is unchanged.",
            )
            .category("Astro")
            .pure()
            .inputs([
                dir_input("Darks", "dark frames"),
                dir_input("Flats", "flat frames"),
                dir_input("Bias", "bias frames"),
                dir_input("Flat Darks", "flat-dark frames"),
            ])
            .input(
                FuncInput::required("Sigma", DataType::Float)
                    .description("Sigma-clipping rejection threshold when stacking.")
                    .default(DEFAULT_SIGMA_THRESHOLD as f64),
            )
            .input(
                FuncInput::required("Cache", DataType::Bool)
                    .description("Write each master next to its frames and reuse it next run.")
                    .default(true),
            )
            .output(
                FuncOutput::new("Masters", MASTERS_DATA_TYPE.clone())
                    .description("Calibration masters for the wired roles."),
            )
            .lambda(FuncLambda::new(move |ctx, _, _, inputs, _, outputs| {
                let cancel = ctx.cancel_flag();
                Box::pin(async move {
                    debug_assert_eq!(inputs.len(), 6);
                    debug_assert_eq!(outputs.len(), 1);

                    let dir = |index: usize| inputs[index].value.as_fs_path().map(PathBuf::from);
                    let dirs = [dir(0), dir(1), dir(2), dir(3)];
                    let sigma = inputs[4]
                        .value
                        .as_f64()
                        .map(|value| value as f32)
                        .expect("sigma input type is validated at the compile boundary");
                    let cache = inputs[5]
                        .value
                        .as_bool()
                        .expect("cache input type is validated at the compile boundary");

                    let masters = runtime::run_cancellable(cancel, move |cancel| {
                        build_masters_cached(dirs, sigma, cache, cancel)
                    })
                    .await?;
                    outputs[0] = DynamicValue::from_custom(Masters::from(masters));
                    Ok(())
                })
            })),
    );
}

fn dir_input(name: &str, what: &str) -> FuncInput {
    FuncInput::optional(name, ASTRO_DIR_DATA_TYPE.clone()).description(format!("Folder of {what}."))
}

pub(crate) fn build_masters_cached(
    dirs: [Option<PathBuf>; 4],
    sigma: f32,
    cache: bool,
    cancel: CancelToken,
) -> Result<CalibrationMasters, BuildMastersError> {
    let [darks, flats, bias, flat_darks] = dirs;
    let role = |dir: Option<PathBuf>,
                config: StackConfig,
                file: &str|
     -> Result<Option<CfaImage>, BuildMastersError> {
        if cancel.is_cancelled() {
            return Err(BuildMastersError::Cancelled);
        }
        let Some(dir) = dir else {
            return Ok(None);
        };
        let frames = astro_io::raw_frame_files(&dir)?;
        let source_key = frame_set_key(&frames)?;
        let cache_path = dir.join(file);
        let marker_path = cache_marker_path(&cache_path);

        if cache {
            match fs::read_to_string(&marker_path).ok().as_deref() {
                Some(marker) if marker == format!("{CACHE_ABSENT}{source_key}") => return Ok(None),
                Some(marker)
                    if marker == format!("{CACHE_PRESENT}{source_key}") && cache_path.is_file() =>
                {
                    let context = LoadContext {
                        cancel: cancel.clone(),
                        ..Default::default()
                    };
                    match CfaImage::from_file(&cache_path, &context) {
                        Ok(master) => return Ok(Some(master)),
                        Err(error) => tracing::warn!(
                            path = %cache_path.display(),
                            %error,
                            "failed to load calibration master cache; rebuilding from source frames"
                        ),
                    }
                }
                _ => {}
            }
        }

        let master = stack_cfa_master(&frames, config, cancel.clone())?;
        if cache {
            let marker = if let Some(master) = &master {
                master
                    .save_fits(&cache_path)
                    .map_err(|source| BuildMastersError::Cache {
                        path: cache_path.clone(),
                        source,
                    })?;
                format!("{CACHE_PRESENT}{source_key}")
            } else {
                remove_cache_file(&cache_path).map_err(|source| BuildMastersError::Cache {
                    path: cache_path.clone(),
                    source,
                })?;
                format!("{CACHE_ABSENT}{source_key}")
            };
            file_utils::publish_bytes(&marker_path, marker.as_bytes(), PublicationMode::Cache)
                .map_err(|source| BuildMastersError::Cache {
                    path: marker_path,
                    source,
                })?;
        }
        Ok(master)
    };

    CalibrationMasters::from_images(
        CalibrationSet {
            dark: role(darks, StackConfig::dark(), "master_dark.fits")?,
            flat: role(flats, StackConfig::flat(), "master_flat.fits")?,
            bias: role(bias, StackConfig::bias(), "master_bias.fits")?,
            flat_dark: role(flat_darks, StackConfig::dark(), "master_flat_dark.fits")?,
        },
        sigma,
        cancel,
    )
    .map_err(BuildMastersError::from)
}

pub(crate) fn frame_set_key(frames: &[PathBuf]) -> Result<String, FrameSetKeyError> {
    let mut hasher = blake3::Hasher::new();
    for frame in frames {
        let name = frame
            .file_name()
            .expect("raw frame path has a file name")
            .as_encoded_bytes();
        let metadata = fs::metadata(frame).map_err(|source| FrameSetKeyError::Metadata {
            path: frame.clone(),
            source,
        })?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        hasher.update(&(name.len() as u64).to_le_bytes());
        hasher.update(name);
        hasher.update(&metadata.len().to_le_bytes());
        hasher.update(&modified.to_le_bytes());
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub(crate) fn cache_marker_path(cache_path: &Path) -> PathBuf {
    let mut name = cache_path
        .file_name()
        .expect("master cache path has a file name")
        .to_os_string();
    name.push(".source");
    cache_path.with_file_name(name)
}

fn remove_cache_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
