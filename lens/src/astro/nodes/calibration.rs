//! Calibration-master node and source-aware on-disk master cache.

use scenarium::FuncId;
use scenarium::async_lambda;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use common::file_utils::{self, PublicationMode};
use common::{CancelToken, FileIdentity};
use lumos::ProgressCallback;
use lumos::{
    CalibrationMasters, CalibrationSet, CfaImage, DEFAULT_SIGMA_THRESHOLD, LoadContext, MasterRole,
    Subtractor, stack_cfa_master,
};
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

#[derive(Debug, thiserror::Error)]
enum BuildMastersError {
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    FrameSet(#[from] FrameSetKeyError),
    #[error(transparent)]
    Stack(#[from] lumos::StackError),
    #[error(transparent)]
    Calibration(#[from] lumos::CalibrationError),
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
        Func::new(
            BUILD_MASTERS_FUNC_ID,
            "Build Masters",
            async_lambda!(move |Invocation { ctx, inputs, outputs, .. }| {
                cancel = ctx.cancel_flag(),
            } => {
                debug_assert_eq!(inputs.len(), 6);
                debug_assert_eq!(outputs.len(), 1);

                let frames = |index: usize| {
                    inputs[index].as_fs_paths().map(|paths| {
                        paths.iter().map(PathBuf::from).collect::<Vec<PathBuf>>()
                    })
                };
                let frame_sets = [frames(0), frames(1), frames(2), frames(3)];
                let sigma = inputs[4].required_f64() as f32;
                let cache = inputs[5].required_bool();

                let masters = runtime::run_cancellable(cancel, move |cancel| {
                    build_masters_cached(frame_sets, sigma, cache, &cancel)
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

fn build_masters_cached(
    frame_sets: [Option<Vec<PathBuf>>; 4],
    sigma: f32,
    cache: bool,
    cancel: &CancelToken,
) -> Result<CalibrationMasters, BuildMastersError> {
    let [darks, flats, bias, flat_darks] = frame_sets;
    // A master stacked with a subtractor taken from each frame is keyed by the subtractor's
    // frames too, so a flat cached before its bias changed, or before flats were calibrated per
    // frame, is stacked again.
    let role = |frames: Option<Vec<PathBuf>>,
                role: MasterRole,
                file: &str,
                subtract: Option<(Subtractor<'_>, &[PathBuf])>|
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
        // Keyed only with the cache on: the key is a `stat` per frame, and one that fails would
        // fail a node that never reads the cache.
        let cached = cache
            .then(|| -> Result<_, BuildMastersError> {
                let paths = role_cache_paths(&frames, file)?;
                let mut key = frame_set_key(&frames)?;
                if let Some((_, subtractor_frames)) = subtract {
                    key.push_str("\nsubtract ");
                    key.push_str(&frame_set_key(subtractor_frames)?);
                }
                Ok((paths, key))
            })
            .transpose()?;

        if let Some((cache_paths, expected_marker)) = &cached {
            match fs::read_to_string(&cache_paths.marker).ok().as_deref() {
                Some(marker) if marker == expected_marker && cache_paths.master.is_file() => {
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

        let master = stack_cfa_master(
            &frames,
            role,
            role.stack_config(),
            subtract.map(|(subtractor, _)| subtractor),
            ProgressCallback::default(),
            cancel.clone(),
        )?
        .expect("a non-empty calibration frame set produces a master");
        if let Some((cache_paths, marker)) = cached {
            master
                .save_fits(&cache_paths.master)
                .map_err(|source| BuildMastersError::Cache {
                    path: cache_paths.master.clone(),
                    source,
                })?;
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

    let bias_master = role(bias.clone(), MasterRole::Bias, "master_bias.fits", None)?;
    let flat_dark_master = role(
        flat_darks.clone(),
        MasterRole::FlatDark,
        "master_flat_dark.fits",
        None,
    )?;
    // Each flat takes its flat-dark, else the bias, before the flats are normalized and combined.
    let flat_subtractor = match (&flat_dark_master, &flat_darks, &bias_master, &bias) {
        (Some(master), Some(frames), _, _) => Some((
            Subtractor {
                role: MasterRole::FlatDark,
                master,
            },
            frames.as_slice(),
        )),
        (None, _, Some(master), Some(frames)) => Some((
            Subtractor {
                role: MasterRole::Bias,
                master,
            },
            frames.as_slice(),
        )),
        _ => None,
    };
    let flat_master = role(flats, MasterRole::Flat, "master_flat.fits", flat_subtractor)?;
    CalibrationMasters::from_images(
        CalibrationSet {
            dark: role(darks, MasterRole::Dark, "master_dark.fits", None)?,
            flat: flat_master,
            bias: bias_master,
            flat_dark: flat_dark_master,
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
        hasher.update(&(name.len() as u64).to_le_bytes());
        hasher.update(name);
        hasher.update(&identity.len.to_le_bytes());
        hasher.update(&identity.mtime_ns.to_le_bytes());
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
mod tests {
    use std::fs;
    use std::io;
    use std::path::PathBuf;
    use std::slice;

    use common::TempDir;

    use common::CancelToken;

    use crate::astro::nodes::calibration::{
        BuildMastersError, FrameSetKeyError, build_masters_cached, frame_set_key, role_cache_paths,
    };

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

    #[test]
    fn master_source_key_changes_with_the_frame_set() {
        let dir = TempDir::new("lens-master-source-key");
        let first = dir.join("a.raf");
        let second = dir.join("b.raf");
        fs::write(&first, b"a").unwrap();
        let one_frame = frame_set_key(slice::from_ref(&first)).unwrap();
        assert_eq!(frame_set_key(slice::from_ref(&first)).unwrap(), one_frame);

        fs::write(&second, b"bb").unwrap();
        let two_frames = frame_set_key(&[first.clone(), second.clone()]).unwrap();
        assert_ne!(two_frames, one_frame);
        // The key follows the selection order: a reordered set sums in another
        // order, so its master can differ in the last bits.
        let reordered = frame_set_key(&[second.clone(), first.clone()]).unwrap();
        assert_ne!(reordered, two_frames);
        fs::write(&first, b"aaa").unwrap();
        let edited = frame_set_key(&[first.clone(), second]).unwrap();
        assert_ne!(edited, two_frames);
        assert_ne!(frame_set_key(&[]).unwrap(), edited);

        fs::remove_file(&first).unwrap();
        let error = frame_set_key(slice::from_ref(&first)).unwrap_err();
        assert!(matches!(
            error,
            FrameSetKeyError::Metadata { path, source }
                if path == first && source.kind() == io::ErrorKind::NotFound
        ));
    }

    /// A missing frame fails the key's `stat` with the cache on, and with it off the key is never
    /// computed: the stack is what reports the missing file.
    #[test]
    fn frame_set_key_runs_only_with_the_cache_on() {
        let dir = TempDir::new("lens-master-key-cache-off");
        let missing = dir.join("missing.raf");
        let build = |cache| {
            build_masters_cached(
                [Some(vec![missing.clone()]), None, None, None],
                3.0,
                cache,
                &CancelToken::never(),
            )
            .unwrap_err()
        };
        assert!(matches!(build(true), BuildMastersError::FrameSet(_)));
        assert!(matches!(build(false), BuildMastersError::Stack(_)));
    }
}
