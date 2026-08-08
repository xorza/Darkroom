//! Shared blocking-runtime adapters for astro node implementations.

use common::CancelToken;
use imaginarium::ProcessingContext;
use lumos::{LinearImage, MlError, OpError};
use scenarium::{DynamicValue, InvokeError, InvokeResult};

use crate::image::Image;

pub(crate) async fn run_frame_op<F>(value: DynamicValue, op: F) -> InvokeResult<DynamicValue>
where
    F: FnOnce(&mut LinearImage) -> Result<(), OpError> + Send + 'static,
{
    let planar = image_to_planar(value).map_err(InvokeError::external)?;
    let out = tokio::task::spawn_blocking(move || {
        let mut planar = planar;
        op(&mut planar)?;
        Ok::<_, OpError>(planar)
    })
    .await
    .map_err(InvokeError::external)?
    .map_err(InvokeError::external)?;
    Ok(DynamicValue::from_custom(Image::from(out)))
}

pub(crate) async fn run_ml<R, F>(value: DynamicValue, op: F) -> InvokeResult<R>
where
    F: FnOnce(LinearImage) -> Result<R, MlError> + Send + 'static,
    R: Send + 'static,
{
    let planar = image_to_planar(value).map_err(InvokeError::external)?;
    tokio::task::spawn_blocking(move || op(planar))
        .await
        .map_err(InvokeError::external)?
        .map_err(InvokeError::external)
}

/// The planes the `lumos` ops run on. An input produced by another astro node is already planar and
/// is taken as-is, so a chain of astro nodes never repacks; only an edge coming from the
/// `imaginarium` side converts, and only once.
fn image_to_planar(value: DynamicValue) -> imaginarium::Result<LinearImage> {
    let cpu = ProcessingContext::cpu_only();
    match value.into_custom::<Image>() {
        Ok(image) => image.to_planar(&cpu),
        Err(value) => value
            .as_custom::<Image>()
            .expect("image input type is validated at the compile boundary")
            .make_planar(&cpu)
            .map(|planar| planar.clone()),
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

#[cfg(test)]
pub(super) mod internals {
    use lumos::LinearImage;
    use scenarium::DynamicValue;

    pub(crate) fn image_to_planar(value: DynamicValue) -> imaginarium::Result<LinearImage> {
        super::image_to_planar(value)
    }
}
