//! The bundle the record path hands down.

use crate::gui::frame::requests::Requests;

/// What one level of the record path takes beside its `Ui`: what the level reads, and where it puts
/// what it asks for. A level adds to `ctx` and hands the rest down through [`Self::with`].
///
/// It holds no `Ui`, as an [`InputStage`](crate::gui::frame::input_stage::InputStage) holds its
/// update view: `Panel::show(ui, |ui| ..)` lends the `ui` to each body, so a stage holding one
/// would be built again in each closure. The `ui` stays the first argument, as each palantir widget
/// takes it.
#[derive(Debug)]
pub(crate) struct RecordStage<'a, C> {
    pub(crate) ctx: C,
    pub(crate) out: &'a mut Requests,
}

impl<C> RecordStage<'_, C> {
    /// This stage for the level below, which reads `ctx`.
    pub(crate) const fn with<N>(&mut self, ctx: N) -> RecordStage<'_, N> {
        RecordStage {
            ctx,
            out: &mut *self.out,
        }
    }
}
