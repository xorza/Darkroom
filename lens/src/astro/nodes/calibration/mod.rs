//! Calibration-master node and source-aware on-disk master cache.

use std::io;
use std::path::{Path, PathBuf};

use common::fs::{self, PublicationMode};
use common::{CancelToken, ErrorChain};
use lumos::{
    CalibrationMasters, CalibrationSet, CfaImage, DecodeConfig, IngestConfig, LoadCtx,
    MasterBuildError, MasterCache, MasterRole, MasterSlot, MastersConfig, ProgressMonitor,
    SigmaThreshold, WriteError,
};
use scenarium::{
    DataType, DynamicValue, FloatSpan, Func, FuncId, InputDecl, Invocation, InvokeError,
    LibraryBuilder, OutputDecl, async_lambda,
};

use crate::astro::masters::{MASTERS_DATA_TYPE, Masters};
use crate::astro::nodes::io::astro_raw_paths_type;
use crate::astro::nodes::runtime;

const BUILD_MASTERS_FUNC_ID: FuncId = FuncId::literal("f2f6f1ff-5b10-409c-900f-d6b48750a529");

/// Why a master could not be kept next to its frames, which the build reports and carries on.
#[derive(Debug, thiserror::Error)]
enum CacheError {
    #[error("failed to write cached calibration master '{path}'", path = .path.display())]
    WriteMaster {
        path: PathBuf,
        #[source]
        source: WriteError,
    },
    #[error("failed to update calibration cache '{path}'", path = .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("calibration cache requires one source directory, but '{first}' and '{other}' differ")]
    SourceDirs { first: PathBuf, other: PathBuf },
}

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
    sigma: SigmaThreshold,
    caching: Caching,
}

impl MastersRequest {
    fn build(&self, cancel: &CancelToken) -> Result<CalibrationMasters, MasterBuildError> {
        let cache = SourceDirCache { cancel };
        CalibrationMasters::build(
            CalibrationSet {
                dark: self.frames.dark.as_slice(),
                flat: self.frames.flat.as_slice(),
                bias: self.frames.bias.as_slice(),
                flat_dark: self.frames.flat_dark.as_slice(),
            },
            &MastersConfig {
                hot_sigma: self.sigma,
                cache: match self.caching {
                    Caching::Off => None,
                    Caching::BesideFrames => Some(&cache),
                },
            },
            &IngestConfig::default(),
            ProgressMonitor::from(cancel.clone()),
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
    fn beside(frames: &[&Path], master_file: &str) -> Result<Self, CacheError> {
        let first = frames.first().expect("empty frame sets are not cached");
        let dir = first.parent().unwrap_or_else(|| Path::new(""));
        if let Some(other) = frames
            .iter()
            .skip(1)
            .find(|path| path.parent().unwrap_or_else(|| Path::new("")) != dir)
        {
            return Err(CacheError::SourceDirs {
                first: first.to_path_buf(),
                other: other.to_path_buf(),
            });
        }
        let master = dir.join(master_file);
        let marker = cache_marker_path(&master);
        Ok(Self { master, marker })
    }
}

pub(crate) fn register(library: &mut LibraryBuilder) {
    library.add(
        Func::builder(
            BUILD_MASTERS_FUNC_ID,
            "Build Masters",
            async_lambda!(move |Invocation { ctx, inputs, outputs, .. }| {
                cancel = ctx.cancel_flag(),
            } => {
                debug_assert_eq!(inputs.len(), 6);
                debug_assert_eq!(outputs.len(), 1);

                let frames = |idx: usize| {
                    inputs[idx]
                        .as_fs_paths()
                        .map_or_default(|paths| paths.iter().map(PathBuf::from).collect())
                };
                let request = MastersRequest {
                    frames: CalibrationSet {
                        dark: frames(0),
                        flat: frames(1),
                        bias: frames(2),
                        flat_dark: frames(3),
                    },
                    sigma: {
                        let sigma = inputs[4].required_f64();
                        SigmaThreshold::new(sigma as f32).map_err(|_refused| {
                            InvokeError::invalid_input(4, "a finite σ of at least 1", sigma)
                        })?
                    },
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
        .input(1, frames_input("Darks", "dark frames"))
        .input(2, frames_input("Flats", "flat frames"))
        .input(3, frames_input("Bias", "bias frames"))
        .input(4, frames_input("Flat Darks", "flat-dark frames"))
        .input(
            5,
            InputDecl::required("Sigma", DataType::Float)
                .description(
                    "Hot-pixel defect threshold: a master-dark pixel more than this many sigma \
                     above its color's dark background is a defect.",
                )
                .float_span(FloatSpan::at_least(1.0))
                .default(f64::from(SigmaThreshold::DEFAULT.get())),
        )
        .input(
            6,
            InputDecl::required("Cache", DataType::Bool)
                .description("Write each master next to its frames and reuse it next run.")
                .default(true),
        )
        .output(
            1,
            OutputDecl::new("Masters", MASTERS_DATA_TYPE)
                .description("Calibration masters for the wired roles."),
        )
        .build(),
    );
}

fn frames_input(name: &str, what: &str) -> InputDecl {
    InputDecl::optional(name, astro_raw_paths_type())
        .description(format!("Camera-RAW {what} to stack."))
}

/// Each master kept next to its frames, which must share one directory, beside a marker that
/// holds the [`MasterKey`](lumos::MasterKey) lumos keyed it by: its frames, the masters subtracted
/// from them, and lumos's recipe for it, so a flat cached before its bias changed, or by an older
/// lumos, is stacked again. A frame set that spans directories is not cached, and the build says
/// so and carries on.
#[derive(Debug)]
struct SourceDirCache<'a> {
    cancel: &'a CancelToken,
}

impl SourceDirCache<'_> {
    /// The paths `slot`'s master is kept at.
    fn paths(slot: &MasterSlot<'_>) -> Result<RoleCachePaths, CacheError> {
        let file = match slot.role {
            MasterRole::Dark => "master_dark.fits",
            MasterRole::Flat => "master_flat.fits",
            MasterRole::Bias => "master_bias.fits",
            MasterRole::FlatDark => "master_flat_dark.fits",
        };
        RoleCachePaths::beside(slot.frames, file)
    }

    /// The marker goes first, so a store that fails before its own marker is written leaves the
    /// master unmarked, never under the key of the master it replaced.
    fn try_store(slot: &MasterSlot<'_>, master: &CfaImage) -> Result<(), CacheError> {
        let paths = Self::paths(slot)?;
        match fs::remove_file(&paths.marker) {
            Err(source) if source.kind() != io::ErrorKind::NotFound => {
                return Err(CacheError::Write {
                    path: paths.marker,
                    source,
                });
            }
            _ => {}
        }
        master
            .write_fits(&paths.master)
            .map_err(|source| CacheError::WriteMaster {
                path: paths.master.clone(),
                source,
            })?;
        fs::publish_bytes(
            &paths.marker,
            slot.key.to_string().as_bytes(),
            PublicationMode::Cache,
        )
        .map_err(|source| CacheError::Write {
            path: paths.marker,
            source,
        })
    }
}

impl MasterCache for SourceDirCache<'_> {
    fn load(&self, slot: &MasterSlot<'_>) -> Option<CfaImage> {
        let paths = Self::paths(slot)
            .map_err(|error| {
                tracing::warn!(
                    error = %ErrorChain::new(&error),
                    "calibration master not cached"
                );
            })
            .ok()?;
        let marked =
            fs::read_to_string(&paths.marker).is_ok_and(|marker| marker == slot.key.to_string());
        if !(marked && fs::is_file(&paths.master)) {
            return None;
        }
        let ctx = LoadCtx::new(DecodeConfig::default(), self.cancel.clone());
        ctx.cfa(&paths.master)
            .map_err(|error| {
                tracing::warn!(
                    path = %paths.master.display(),
                    error = %ErrorChain::new(&error),
                    "failed to load calibration master cache; rebuilding from source frames"
                );
            })
            .ok()
    }

    fn store(&self, slot: &MasterSlot<'_>, master: &CfaImage) {
        if let Err(error) = Self::try_store(slot, master) {
            tracing::warn!(
                error = %ErrorChain::new(&error),
                "failed to keep a calibration master; the build uses it as stacked"
            );
        }
    }
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
