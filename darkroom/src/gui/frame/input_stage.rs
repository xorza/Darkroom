//! The bundle the update path hands down.

use palantir::UpdateCtx;

use crate::gui::frame::requests::Requests;

/// What one level of the update path takes: the frame's input, what the level reads, and where it
/// puts what it asks for. A level adds to `ctx` and hands the rest down through [`Self::with`].
///
/// A stage lasts one phase, as a drain between phases needs the document its context reads. A
/// settle ahead of a record takes one too, as it reads only this view of the `Ui`. The fields are
/// public so a function borrows them apart, which a method on the stage could not. Refused: `out`
/// inside the `Copy` context chain, which takes `Copy` from every context; a struct per level, six
/// names for one shape.
#[derive(Debug)]
pub(crate) struct InputStage<'a, 'u, C> {
    pub(crate) update: &'a mut UpdateCtx<'u>,
    pub(crate) ctx: C,
    pub(crate) out: &'a mut Requests,
}

impl<'u, C> InputStage<'_, 'u, C> {
    /// This stage for the level below, which reads `ctx`.
    pub(crate) const fn with<N>(&mut self, ctx: N) -> InputStage<'_, 'u, N> {
        InputStage {
            update: &mut *self.update,
            ctx,
            out: &mut *self.out,
        }
    }
}
