//! Calibration-master node and source-aware on-disk master cache.

use scenarium::FuncId;
use scenarium::async_lambda;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use common::file_utils::{self, PublicationMode};
use common::{CancelToken, FileIdentity};
use lumos::{
    CachedMaster, CalibrationMasters, CalibrationSet, CfaImage, DEFAULT_SIGMA_THRESHOLD,
    IngestConfig, LoadContext, MasterBuildError, MasterCache, MasterInputs, MasterRole,
};
use lumos::{FitsWriteError, ProgressCallback, ProgressMonitor};
use scenarium::Invocation;
use scenarium::{DataType, DynamicValue, Func, FuncInput, FuncOutput, Library};

use crate::astro::masters::{MASTERS_DATA_TYPE, Masters};
use crate::astro::nodes::io::ASTRO_RAW_PATHS_DATA_TYPE;
use crate::astro::nodes::runtime;

const BUILD_MASTERS_FUNC_ID: FuncId = FuncId::literal("f2f6f1ff-5b10-409c-900f-d6b48750a529");

#[derive(Debug, thiserror::Error)]
enum FrameSetKeyError {
    #[error("failed to read metadata for '{path}': {source}", path = .path.display())]
    Metadata {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Why a master kept next to its frames could not be read or written.
#[derive(Debug, thiserror::Error)]
enum CacheError {
    #[error(transparent)]
    FrameSet(#[from] FrameSetKeyError),
    #[error("failed to write cached calibration master '{path}': {source}", path = .path.display())]
    WriteMaster {
        path: PathBuf,
        #[source]
        source: FitsWriteError,
    },
    #[error("failed to update calibration cache '{path}': {source}", path = .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("calibration cache requires one source directory, but '{first}' and '{other}' differ")]
    SourceDirectories { first: PathBuf, other: PathBuf },
}

type BuildMastersError = MasterBuildError<CacheError>;

/// Whether Build Masters keeps each master beside its frames, to reuse while they are unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Caching {
    Off,
    BesideFrames,
}

/// What Build Masters stacks: the frames of each role, none for a role left unwired, the defect
/// threshold, and where the masters are kept.
#[derive(Debug)]
struct MastersRequest {
    frames: CalibrationSet<Vec<PathBuf>>,
    sigma: f32,
    caching: Caching,
}

impl MastersRequest {
    fn build(&self, cancel: &CancelToken) -> Result<CalibrationMasters, BuildMastersError> {
        CalibrationMasters::build(
            CalibrationSet {
                dark: self.frames.dark.as_slice(),
                flat: self.frames.flat.as_slice(),
                bias: self.frames.bias.as_slice(),
                flat_dark: self.frames.flat_dark.as_slice(),
            },
            self.sigma,
            &IngestConfig::default(),
            &SourceDirectoryCache {
                caching: self.caching,
                cancel,
            },
            &ProgressMonitor::new(ProgressCallback::default(), cancel.clone()),
        )
    }
}

#[derive(Debug)]
struct RoleCachePaths {
    master: PathBuf,
    marker: PathBuf,
}

impl RoleCachePaths {
    /// The master named `master_file` in the one directory `frames` share, and its marker beside
    /// it.
    fn beside(frames: &[PathBuf], master_file: &str) -> Result<Self, CacheError> {
        let first = frames.first().expect("empty frame sets are not cached");
        let directory = first.parent().unwrap_or_else(|| Path::new(""));
        if let Some(other) = frames
            .iter()
            .skip(1)
            .find(|path| path.parent().unwrap_or_else(|| Path::new("")) != directory)
        {
            return Err(CacheError::SourceDirectories {
                first: first.clone(),
                other: other.clone(),
            });
        }
        let master = directory.join(master_file);
        let marker = cache_marker_path(&master);
        Ok(Self { master, marker })
    }
}

pub(crate) fn register(library: &mut Library) {
    library.add(
        Func::new(
            BUILD_MASTERS_FUNC_ID,
            "Build Masters",
            async_lambda!(move |Invocation { ctx, inputs, outputs, .. }| {
                cancel = ctx.cancel_flag(),
            } => {
                debug_assert_eq!(inputs.len(), 6);
                debug_assert_eq!(outputs.len(), 1);

                let frames = |index: usize| {
                    inputs[index]
                        .as_fs_paths()
                        .map(|paths| paths.iter().map(PathBuf::from).collect())
                        .unwrap_or_default()
                };
                let request = MastersRequest {
                    frames: CalibrationSet {
                        dark: frames(0),
                        flat: frames(1),
                        bias: frames(2),
                        flat_dark: frames(3),
                    },
                    sigma: inputs[4].required_f64() as f32,
                    caching: if inputs[5].required_bool() {
                        Caching::BesideFrames
                    } else {
                        Caching::Off
                    },
                };

                let masters = runtime::run_cancellable(cancel, move |cancel| {
                    request.build(&cancel)
                })
                .await?;
                outputs[0] = DynamicValue::from_custom(Masters::from(masters));
                Ok(())
            }),
        )
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
                .description(
                    "Hot-pixel defect threshold: a master-dark pixel more than this many sigma \
                     above its color's dark background is a defect. Values below 1 count as 1.",
                )
                .default(f64::from(DEFAULT_SIGMA_THRESHOLD)),
        )
        .input(
            FuncInput::required("Cache", DataType::Bool)
                .description("Write each master next to its frames and reuse it next run.")
                .default(true),
        )
        .output(
            FuncOutput::new("Masters", MASTERS_DATA_TYPE)
                .description("Calibration masters for the wired roles."),
        ),
    );
}

fn frames_input(name: &str, what: &str) -> FuncInput {
    FuncInput::optional(name, ASTRO_RAW_PATHS_DATA_TYPE.clone())
        .description(format!("Camera-RAW {what} to stack."))
}

/// Each master kept next to its frames, which must share one directory, beside a marker that keys
/// it by them, by the frames of the master subtracted from each, and by lumos's recipe for it, so a
/// flat cached before its bias changed, or by an older lumos, is stacked again. Off, it keeps
/// nothing and keys nothing: a key is a `stat` per frame, and one that fails would fail a build
/// that never reads the cache.
#[derive(Debug)]
struct SourceDirectoryCache<'a> {
    caching: Caching,
    cancel: &'a CancelToken,
}

/// Where a role's master is kept, and the marker its frames key it by as they were read before
/// the stack.
#[derive(Debug)]
struct CachePlace {
    paths: RoleCachePaths,
    key: String,
}

impl CachePlace {
    /// The place of `inputs`' master, keyed by its frames as they are now.
    fn of(inputs: &MasterInputs<'_, PathBuf>) -> Result<Self, CacheError> {
        let file = match inputs.role {
            MasterRole::Dark => "master_dark.fits",
            MasterRole::Flat => "master_flat.fits",
            MasterRole::Bias => "master_bias.fits",
            MasterRole::FlatDark => "master_flat_dark.fits",
        };
        let paths = RoleCachePaths::beside(inputs.frames, file)?;
        let mut key = frame_set_key(inputs.frames)?;
        key.push_str("\nrecipe ");
        key.push_str(&inputs.recipe.to_string());
        if let Some(subtracted) = &inputs.subtracted {
            key.push_str("\nsubtract ");
            key.push_str(&frame_set_key(subtracted.frames)?);
        }
        Ok(Self { paths, key })
    }
}

impl MasterCache<PathBuf> for SourceDirectoryCache<'_> {
    type Error = CacheError;
    /// `None` with the cache off.
    type Key = Option<CachePlace>;

    fn load(
        &self,
        inputs: &MasterInputs<'_, PathBuf>,
    ) -> Result<CachedMaster<Option<CachePlace>>, CacheError> {
        if self.caching == Caching::Off {
            return Ok(CachedMaster::Missing(None));
        }
        let place = CachePlace::of(inputs)?;
        let marked =
            fs::read_to_string(&place.paths.marker).is_ok_and(|marker| marker == place.key);
        if marked && place.paths.master.is_file() {
            let context = LoadContext {
                cancel: self.cancel.clone(),
                ..Default::default()
            };
            match CfaImage::from_file(&place.paths.master, &context) {
                Ok(master) => return Ok(CachedMaster::Kept(Box::new(master))),
                Err(error) => tracing::warn!(
                    path = %place.paths.master.display(),
                    %error,
                    "failed to load calibration master cache; rebuilding from source frames"
                ),
            }
        }
        Ok(CachedMaster::Missing(Some(place)))
    }

    fn store(&self, place: Option<CachePlace>, master: &CfaImage) -> Result<(), CacheError> {
        let Some(CachePlace { paths, key }) = place else {
            return Ok(());
        };
        master
            .save_fits(&paths.master)
            .map_err(|source| CacheError::WriteMaster {
                path: paths.master.clone(),
                source,
            })?;
        file_utils::publish_bytes(&paths.marker, key.as_bytes(), PublicationMode::Cache).map_err(
            |source| CacheError::Write {
                path: paths.marker,
                source,
            },
        )
    }
}

fn frame_set_key(frames: &[PathBuf]) -> Result<String, FrameSetKeyError> {
    let mut hasher = blake3::Hasher::new();
    for frame in frames {
        let name = frame
            .file_name()
            .expect("raw frame path has a file name")
            .as_encoded_bytes();
        let identity = FileIdentity::of(frame).map_err(|source| FrameSetKeyError::Metadata {
            path: frame.clone(),
            source,
        })?;
        hasher.update(
            &u64::try_from(name.len())
                .expect("a file name's length fits u64")
                .to_le_bytes(),
        );
        hasher.update(name);
        identity.write_to(|bytes| {
            hasher.update(bytes);
        });
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn cache_marker_path(cache_path: &Path) -> PathBuf {
    let mut name = cache_path
        .file_name()
        .expect("master cache path has a file name")
        .to_os_string();
    name.push(".source");
    cache_path.with_file_name(name)
}

#[cfg(test)]
mod tests;
