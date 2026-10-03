//! Shared blocking-runtime adapters for astro node implementations.

use common::CancelToken;
use lumos::{LinearImage, OpError};
use scenarium::{DynamicValue, InvokeError, InvokeResult};

use crate::image::Image;
use std::error;
use tokio::task;

/// An in-place `lumos` op over the planes of a required image input, on the
/// blocking pool, handing back the image it leaves.
pub(crate) async fn run_frame_op<F>(value: DynamicValue, op: F) -> InvokeResult<DynamicValue>
where
    F: FnOnce(&mut LinearImage) -> Result<(), OpError> + Send + 'static,
{
    run_on_planes(value, move |mut planar| {
        op(&mut planar)?;
        Ok::<_, OpError>(DynamicValue::from_custom(Image::from(planar)))
    })
    .await
}

/// `op` over the planes of a required image input, on the blocking pool.
pub(crate) async fn run_on_planes<R, E, F>(value: DynamicValue, op: F) -> InvokeResult<R>
where
    F: FnOnce(LinearImage) -> Result<R, E> + Send + 'static,
    E: error::Error + Send + Sync + 'static,
    R: Send + 'static,
{
    let planar = Image::take_planar(value);
    task::spawn_blocking(move || op(planar))
        .await
        .map_err(InvokeError::external)?
        .map_err(InvokeError::external)
}

pub(crate) async fn run_cancellable<T, E, F>(cancel: CancelToken, op: F) -> InvokeResult<T>
where
    E: error::Error + Send + Sync + 'static,
    F: FnOnce(CancelToken) -> Result<T, E> + Send + 'static,
    T: Send + 'static,
{
    let cancel_for_op = cancel.clone();
    match task::spawn_blocking(move || op(cancel_for_op))
        .await
        .map_err(InvokeError::external)?
    {
        Ok(value) => Ok(value),
        Err(_) if cancel.is_cancelled() => Err(InvokeError::Cancelled),
        Err(error) => Err(InvokeError::external(error)),
    }
}
