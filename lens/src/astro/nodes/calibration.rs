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
use crate::astro::nodes::io::ASTRO_RAW_PATHS_DATA_TYPE;
use crate::astro::nodes::runtime;

const CACHE_PRESENT: &str = "present:";

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
    FrameSet(#[from] FrameSetKeyError),
    #[error(transparent)]
    Stack(#[from] lumos::StackError),
    #[error("failed to update calibration cache '{path}': {source}", path = .path.display())]
    Cache {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("calibration cache requires one source directory, but '{first}' and '{other}' differ")]
    CacheSourceDirectories { first: PathBuf, other: PathBuf },
}

#[derive(Debug)]
struct RoleCachePaths {
    master: PathBuf,
    marker: PathBuf,
}

pub(crate) fn register(library: &mut Library) {
    library.add(
        Func::new("f2f6f1ff-5b10-409c-900f-d6b48750a529", "Build Masters")
            .description(
                "Stacks selected raw calibration frames (darks/flats/bias/flat-darks) into \
                 calibration masters. With `cache` on, each master is written next to its \
                 same-directory source frames and reused while that selection is unchanged.",
            )
            .category("Astro")
            .pure()
            .inputs([
                frames_input("Darks", "dark frames"),
                frames_input("Flats", "flat frames"),
                frames_input("Bias", "bias frames"),
                frames_input("Flat Darks", "flat-dark frames"),
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

                    let frames = |index: usize| {
                        inputs[index]
                            .value
                            .as_fs_paths()
                            .map(|paths| paths.iter().map(PathBuf::from).collect::<Vec<PathBuf>>())
                    };
                    let frame_sets = [frames(0), frames(1), frames(2), frames(3)];
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
                        build_masters_cached(frame_sets, sigma, cache, cancel)
                    })
                    .await?;
                    outputs[0] = DynamicValue::from_custom(Masters::from(masters));
                    Ok(())
                })
            })),
    );
}

fn frames_input(name: &str, what: &str) -> FuncInput {
    FuncInput::optional(name, ASTRO_RAW_PATHS_DATA_TYPE.clone())
        .description(format!("Camera-RAW {what} to stack."))
}

pub(crate) fn build_masters_cached(
    frame_sets: [Option<Vec<PathBuf>>; 4],
    sigma: f32,
    cache: bool,
    cancel: CancelToken,
) -> Result<CalibrationMasters, BuildMastersError> {
    let [darks, flats, bias, flat_darks] = frame_sets;
    let role = |frames: Option<Vec<PathBuf>>,
                config: StackConfig,
                file: &str|
     -> Result<Option<CfaImage>, BuildMastersError> {
        if cancel.is_cancelled() {
            return Err(BuildMastersError::Cancelled);
        }
        let Some(frames) = frames else {
            return Ok(None);
        };
        if frames.is_empty() {
            return Ok(None);
        }
        let source_key = frame_set_key(&frames)?;
        let cache_paths = cache.then(|| role_cache_paths(&frames, file)).transpose()?;

        if let Some(cache_paths) = &cache_paths {
            match fs::read_to_string(&cache_paths.marker).ok().as_deref() {
                Some(marker)
                    if marker == format!("{CACHE_PRESENT}{source_key}")
                        && cache_paths.master.is_file() =>
                {
                    let context = LoadContext {
                        cancel: cancel.clone(),
                        ..Default::default()
                    };
                    match CfaImage::from_file(&cache_paths.master, &context) {
                        Ok(master) => return Ok(Some(master)),
                        Err(error) => tracing::warn!(
                            path = %cache_paths.master.display(),
                            %error,
                            "failed to load calibration master cache; rebuilding from source frames"
                        ),
                    }
                }
                _ => {}
            }
        }

        let master = stack_cfa_master(&frames, config, cancel.clone())?
            .expect("a non-empty calibration frame set produces a master");
        if let Some(cache_paths) = cache_paths {
            master
                .save_fits(&cache_paths.master)
                .map_err(|source| BuildMastersError::Cache {
                    path: cache_paths.master.clone(),
                    source,
                })?;
            let marker = format!("{CACHE_PRESENT}{source_key}");
            file_utils::publish_bytes(
                &cache_paths.marker,
                marker.as_bytes(),
                PublicationMode::Cache,
            )
            .map_err(|source| BuildMastersError::Cache {
                path: cache_paths.marker,
                source,
            })?;
        }
        Ok(Some(master))
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

fn role_cache_paths(
    frames: &[PathBuf],
    master_file: &str,
) -> Result<RoleCachePaths, BuildMastersError> {
    let first = frames.first().expect("empty frame sets are not cached");
    let directory = first.parent().unwrap_or_else(|| Path::new(""));
    if let Some(other) = frames
        .iter()
        .skip(1)
        .find(|path| path.parent().unwrap_or_else(|| Path::new("")) != directory)
    {
        return Err(BuildMastersError::CacheSourceDirectories {
            first: first.clone(),
            other: other.clone(),
        });
    }
    let master = directory.join(master_file);
    let marker = cache_marker_path(&master);
    Ok(RoleCachePaths { master, marker })
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::astro::nodes::calibration::{BuildMastersError, role_cache_paths};

    #[test]
    fn role_cache_requires_one_source_directory() {
        let frames = [
            PathBuf::from("calibration/darks/a.raf"),
            PathBuf::from("calibration/darks/b.raf"),
        ];
        let paths = role_cache_paths(&frames, "master_dark.fits").unwrap();
        assert_eq!(
            paths.master,
            PathBuf::from("calibration/darks/master_dark.fits")
        );
        assert_eq!(
            paths.marker,
            PathBuf::from("calibration/darks/master_dark.fits.source")
        );

        let mixed = [
            PathBuf::from("calibration/darks/a.raf"),
            PathBuf::from("archive/darks/b.raf"),
        ];
        let error = role_cache_paths(&mixed, "master_dark.fits").unwrap_err();
        assert!(matches!(
            error,
            BuildMastersError::CacheSourceDirectories {
                first,
                other
            } if first == mixed[0] && other == mixed[1]
        ));
    }
}
