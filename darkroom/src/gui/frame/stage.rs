//! The bundle the input stage and the record path hand down beside the `Ui`.

use crate::gui::frame::requests::Requests;

/// What one level takes beside its `Ui`: what the level reads, and where it puts what it asks for. A
/// level adds to `ctx` and hands the rest down through [`Self::with`].
///
/// It holds no `Ui`: `Panel::show(ui, |ui| ..)` lends the `ui` to each body, so a stage holding one
/// would be built again in each closure. The `ui` stays the first argument, as each palantir widget
/// takes it. A stage lasts one phase, as a drain between phases needs the document its context
/// reads. The fields are public so a function borrows them apart, which a method on the stage could
/// not. Refused: `out` inside the `Copy` context chain, which takes `Copy` from every context; a
/// struct per level, six names for one shape.
#[derive(Debug)]
pub(crate) struct Stage<'a, C> {
    pub(crate) ctx: C,
    pub(crate) out: &'a mut Requests,
}

impl<C> Stage<'_, C> {
    /// This stage for the level below, which reads `ctx`.
    pub(crate) const fn with<N>(&mut self, ctx: N) -> Stage<'_, N> {
        Stage {
            ctx,
            out: &mut *self.out,
        }
    }
}
