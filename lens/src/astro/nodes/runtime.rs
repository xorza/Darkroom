//! Shared blocking-runtime adapters for astro node implementations.

use common::CancelToken;
use imaginarium::{Image as RawImage, ProcessingContext};
use lumos::{MlError, OpError};
use scenarium::{DynamicValue, InvokeError, InvokeResult};

use crate::image::Image;

pub(crate) async fn run_frame_op<F>(value: DynamicValue, op: F) -> InvokeResult<DynamicValue>
where
    F: FnOnce(&mut RawImage) -> Result<(), OpError> + Send + 'static,
{
    let cpu = image_to_cpu(value).map_err(InvokeError::external)?;
    let out = tokio::task::spawn_blocking(move || {
        let mut cpu = cpu;
        op(&mut cpu)?;
        Ok::<_, OpError>(cpu)
    })
    .await
    .map_err(InvokeError::external)?
    .map_err(InvokeError::external)?;
    Ok(DynamicValue::from_custom(Image::from(out)))
}

pub(crate) async fn run_ml<R, F>(value: DynamicValue, op: F) -> InvokeResult<R>
where
    F: FnOnce(RawImage) -> Result<R, MlError> + Send + 'static,
    R: Send + 'static,
{
    let cpu = image_to_cpu(value).map_err(InvokeError::external)?;
    tokio::task::spawn_blocking(move || op(cpu))
        .await
        .map_err(InvokeError::external)?
        .map_err(InvokeError::external)
}

pub(crate) fn image_to_cpu(value: DynamicValue) -> imaginarium::Result<RawImage> {
    let cpu = ProcessingContext::cpu_only();
    match value.into_custom::<Image>() {
        Ok(image) => image.buffer.to_cpu(&cpu),
        Err(value) => {
            let image = value
                .as_custom::<Image>()
                .expect("image input type is validated at the compile boundary");
            Ok(image.buffer.make_cpu(&cpu)?.clone())
        }
    }
}

pub(crate) async fn run_cancellable<T, E, F>(cancel: CancelToken, op: F) -> InvokeResult<T>
where
    E: std::error::Error + Send + Sync + 'static,
    F: FnOnce(CancelToken) -> Result<T, E> + Send + 'static,
    T: Send + 'static,
{
    let cancel_for_op = cancel.clone();
    match tokio::task::spawn_blocking(move || op(cancel_for_op))
        .await
        .map_err(InvokeError::external)?
    {
        Ok(value) => Ok(value),
        Err(_) if cancel.is_cancelled() => Err(InvokeError::Cancelled),
        Err(error) => Err(InvokeError::external(error)),
    }
}
