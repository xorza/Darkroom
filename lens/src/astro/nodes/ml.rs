//! ONNX-backed denoise and star-removal nodes.

use std::mem;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use common::FileIdentity;
use lumos::{MlDenoise, MlError, OnnxModel, RemoveStars};
use scenarium::{
    AnyState, ConstValue, DataType, DynamicValue, FsPathConfig, FsPathMode, Func, FuncId,
    FuncInput, FuncOutput, Invocation, InvokeError, Library, async_lambda,
};

use crate::astro::nodes::{MlModelPaths, runtime};
use crate::image::{IMAGE_DATA_TYPE, Image};

const DENOISE_FUNC_ID: FuncId = FuncId::literal("ace786f9-8a02-4ed1-93a0-ad67bf0680f8");
const STAR_REMOVAL_FUNC_ID: FuncId = FuncId::literal("60c31a76-eed4-467c-9ba3-5c89d294a91b");

pub(crate) fn register(library: &mut Library, model_paths: &MlModelPaths) {
    register_denoise(library, &model_paths.denoise);
    register_star_removal(library, &model_paths.star_removal);
}

fn register_denoise(library: &mut Library, model_path: &Path) {
    library.add(
        Func::builder(
            DENOISE_FUNC_ID,
            "ML Denoise",
            async_lambda!(move |Invocation { ctx, inputs, outputs, state, .. }| {
                cancel = ctx.cancel_flag(),
            } => {
                debug_assert_eq!(inputs.len(), 2);
                debug_assert_eq!(outputs.len(), 1);
                let path = PathBuf::from(inputs[1].required_fs_path());
                let kept = KeptModel::<MlDenoise>::shared(state, &path);
                let (output, kept) =
                    runtime::run_on_planes(mem::take(&mut inputs[0]), move |mut image| {
                        let kept = KeptModel::current(kept, path, denoise)?;
                        let output = kept.lock().apply(&mut image, &cancel).map(|()| image);
                        Ok::<_, MlError>((output, kept))
                    })
                    .await?;
                state.set(kept);
                let output = output.map_err(invoke_error)?;
                outputs[0] = DynamicValue::from_custom(Image::from(output));
                Ok(())
            }),
        )
        .description("Denoises a stretched image with an ONNX model (DeepSNR).")
        .category("Astro")
        .pure()
        .input(Image::input("Image"))
        .input(model_input("Model", model_path))
        .output(FuncOutput::new("Image", IMAGE_DATA_TYPE).description("Processed image."))
        .build(),
    );
}

fn register_star_removal(library: &mut Library, model_path: &Path) {
    library.add(
        Func::builder(
            STAR_REMOVAL_FUNC_ID,
            "ML Star Removal",
            async_lambda!(move |Invocation { ctx, inputs, demand: output_demand, outputs, state, .. }| {
                cancel = ctx.cancel_flag(),
                need_stars = !output_demand[1].is_skip(),
            } => {
                debug_assert_eq!(inputs.len(), 2);
                debug_assert_eq!(outputs.len(), 2);
                let path = PathBuf::from(inputs[1].required_fs_path());
                let kept = KeptModel::<RemoveStars>::shared(state, &path);
                if need_stars {
                    let (result, kept) =
                        runtime::run_on_planes(mem::take(&mut inputs[0]), move |image| {
                            let kept = KeptModel::current(kept, path, star_removal)?;
                            let result = kept.lock().split(image, &cancel);
                            Ok::<_, MlError>((result, kept))
                        })
                        .await?;
                    state.set(kept);
                    let result = result.map_err(invoke_error)?;
                    outputs[0] = DynamicValue::from_custom(Image::from(result.starless));
                    outputs[1] = DynamicValue::from_custom(Image::from(result.stars));
                } else {
                    let (starless, kept) = runtime::run_on_planes(
                        mem::take(&mut inputs[0]),
                        move |mut image| {
                            let kept = KeptModel::current(kept, path, star_removal)?;
                            let starless =
                                kept.lock().apply(&mut image, &cancel).map(|()| image);
                            Ok::<_, MlError>((starless, kept))
                        },
                    )
                    .await?;
                    state.set(kept);
                    let starless = starless.map_err(invoke_error)?;
                    outputs[0] = DynamicValue::from_custom(Image::from(starless));
                }
                Ok(())
            }),
        )
        .description("Removes stars with a StarNet ONNX model (starless + stars).")
        .category("Astro")
        .pure()
        .input(Image::input("Image"))
        .input(model_input("Model", model_path))
        .output(
            FuncOutput::new("Starless", IMAGE_DATA_TYPE)
                .description("The image with stars removed."),
        )
        .output(FuncOutput::new("Stars", IMAGE_DATA_TYPE).description("The recovered star layer."))
        .build(),
    );
}

fn model_input(name: &str, default: &Path) -> FuncInput {
    FuncInput::required(
        name,
        DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
            FsPathMode::ExistingFile,
            vec!["onnx".to_string()],
        ))),
    )
    .description("ONNX model file. Its file identity participates in the node cache key.")
    .default(ConstValue::FsPath(default.display().to_string()))
}

/// A model's failure as the node reports it: a cancel as the run's own, so the executor reads the
/// node as stopped rather than failed.
fn invoke_error(error: MlError) -> InvokeError {
    match error {
        MlError::Cancelled => InvokeError::Cancelled,
        error => InvokeError::external(error),
    }
}

/// The denoise model at `path`, loaded now.
fn denoise(path: &Path) -> Result<MlDenoise, MlError> {
    Ok(MlDenoise::new(OnnxModel::load(path)?))
}

/// The star-removal model at `path`, loaded now.
fn star_removal(path: &Path) -> Result<RemoveStars, MlError> {
    Ok(RemoveStars::new(OnnxModel::load(path)?))
}

/// A model a node loaded, shared with its state while the file it read stays the same: a load
/// reads some 125 MB and builds an ONNX session, which a run of the node with the same file skips.
/// The state keeps its share whatever becomes of the run, a failed one included.
#[derive(Debug)]
struct KeptModel<T> {
    path: PathBuf,
    identity: FileIdentity,
    model: Arc<Mutex<T>>,
}

impl<T: Send + 'static> KeptModel<T> {
    /// A share of the model `state` keeps for `path`; none for another path.
    fn shared(state: &AnyState, path: &Path) -> Option<Self> {
        state
            .get::<Self>()
            .filter(|kept| kept.path == path)
            .map(|kept| Self {
                path: kept.path.clone(),
                identity: kept.identity,
                model: Arc::clone(&kept.model),
            })
    }

    /// `kept` while the file at `path` is the one it read, else what `load` makes of the file now.
    fn current(
        kept: Option<Self>,
        path: PathBuf,
        load: impl FnOnce(&Path) -> Result<T, MlError>,
    ) -> Result<Self, MlError> {
        let identity = FileIdentity::of(&path).map_err(|source| MlError::Read {
            path: path.clone(),
            source,
        })?;
        if let Some(kept) = kept.filter(|kept| kept.identity == identity) {
            return Ok(kept);
        }
        let model = Arc::new(Mutex::new(load(&path)?));
        Ok(Self {
            path,
            identity,
            model,
        })
    }

    /// The model, for one run: a node runs one at a time, so nothing waits on it.
    fn lock(&self) -> MutexGuard<'_, T> {
        self.model
            .lock()
            .expect("a model's run that panicked ended the process")
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use common::{TempDir, fs};
    use lumos::MlError;
    use scenarium::{AnyState, InvokeError};

    use crate::astro::nodes::ml::{KeptModel, invoke_error};

    /// A model's cancel reaches the executor as the run's cancel, and its other failures as the
    /// node's own.
    #[test]
    fn a_cancelled_model_reports_the_run_cancelled() {
        assert!(matches!(
            invoke_error(MlError::Cancelled),
            InvokeError::Cancelled
        ));
        assert!(matches!(
            invoke_error(MlError::OutputLen { len: 1 }),
            InvokeError::External(_)
        ));
    }

    /// A model is loaded once and kept while its file stays the same: a second run of the same
    /// file loads nothing, a rewritten file of another length loads again, and a node asked for
    /// another path shares nothing from its state.
    #[test]
    fn a_kept_model_is_loaded_again_only_when_its_file_changes() {
        let dir = TempDir::new("lens-kept-model");
        let path = dir.join("model.onnx");
        fs::write(&path, b"a").unwrap();
        let loads = Cell::new(0);
        let load = |_: &std::path::Path| {
            loads.set(loads.get() + 1);
            Ok(loads.get())
        };
        let mut state = AnyState::default();
        let run = |state: &mut AnyState| {
            let kept = KeptModel::<u32>::shared(state, &path);
            let kept = KeptModel::current(kept, path.clone(), load).unwrap();
            let model = *kept.lock();
            state.set(kept);
            model
        };
        assert_eq!(run(&mut state), 1);
        assert_eq!(run(&mut state), 1);
        fs::write(&path, b"bb").unwrap();
        assert_eq!(run(&mut state), 2);
        assert!(KeptModel::<u32>::shared(&state, &dir.join("other.onnx")).is_none());
        assert_eq!(loads.get(), 2);
    }
}
