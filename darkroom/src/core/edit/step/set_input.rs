//! Binding, rebinding, and unbinding one input port.

use scenarium::{Binding, InputPort};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;

/// What one input port is bound to, before and after: a wire to a producer,
/// an inline constant, or nothing.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetInput {
    pub(crate) input: InputPort,
    pub(crate) binding: Change<Option<Binding>>,
}

impl Reversible for SetInput {
    fn write(&self, doc: &mut Document, dir: Direction) {
        doc.graph
            .set_input_binding(self.input, self.binding.half(dir).clone());
    }

    fn is_noop(&self) -> bool {
        self.binding.unchanged()
    }

    fn dirties_document(&self) -> bool {
        true
    }

    /// The inline const-value editor exists only while the binding is
    /// `Const(_)`. Flipping that presence (None ⇄ Const, Bind ⇄ Const) adds or
    /// removes a widget, so the node remeasures and every port below it
    /// shifts — connection curves have to re-sample their endpoints. Typing
    /// inside an existing `Const` keeps the editor present at its `Fixed`
    /// size, so a value-only edit costs no second pass.
    fn invalidates_cached_geometry(&self) -> bool {
        let Change { from, to } = &self.binding;
        matches!(from, Some(Binding::Const(_))) != matches!(to, Some(Binding::Const(_)))
    }
}
