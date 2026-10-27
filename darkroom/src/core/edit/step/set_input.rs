//! Binding, rebinding, and unbinding one input port.

use scenarium::{Binding, InputPort};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;
use crate::core::edit::step::set_ties::SetTies;

/// What one input port is bound to, before and after: a wire to a producer,
/// an inline constant, or nothing.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetInput {
    pub(crate) input: InputPort,
    pub(crate) binding: Change<Option<Binding>>,
    /// The route of the wire the edit ends or rewires, and each tie that
    /// only it held: a wire is both of its ends.
    pub(crate) ties: SetTies,
}

impl Reversible for SetInput {
    fn write(&self, document: &mut Document, dir: Direction) {
        document
            .graph
            .set_input_binding(self.input, self.binding.half(dir).clone());
        self.ties.write(document, dir);
    }

    /// The ties change only with the binding.
    fn is_noop(&self) -> bool {
        self.binding.unchanged()
    }

    fn dirties_document(&self) -> bool {
        true
    }

    fn facts(&self) -> EditFacts {
        let Change { from, to } = &self.binding;
        EditFacts {
            binding_kind: matches!(from, Some(Binding::Const(_)))
                != matches!(to, Some(Binding::Const(_))),
            ..EditFacts::NONE
        }
    }
}
