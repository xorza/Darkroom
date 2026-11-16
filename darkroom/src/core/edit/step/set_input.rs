//! Binding, rebinding, and unbinding one input port.

use scenarium::{Binding, GhostSource, GraphId, InputPort, InputState};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;
use crate::core::edit::step::set_ties::SetTies;

/// What one input port holds, before and after: a wire to a producer, an inline constant, a
/// ghost of either, or nothing. An edit never makes a ghost; one that binds or clears an input
/// takes its ghost, and the undo puts it back.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetInput {
    pub(crate) input: InputPort,
    pub(crate) state: Change<InputState>,
    /// The route of the wire the edit ends or rewires, and each tie that only it held.
    pub(crate) ties: SetTies,
}

impl Reversible for SetInput {
    fn write(&self, document: &mut Document, graph: GraphId, dir: Direction) {
        document
            .slot_mut(graph)
            .graph
            .set_input_state(self.input, self.state.half(dir).clone());
        self.ties.write(document, graph, dir);
    }

    /// The ties change only with the binding.
    fn is_noop(&self) -> bool {
        self.state.unchanged()
    }

    fn dirties_document(&self) -> bool {
        true
    }

    fn facts(&self) -> EditFacts {
        // A kept constant shows in the value column as a constant does.
        let constant = |state: &InputState| match state {
            InputState::Live(binding) => matches!(binding, Binding::Const(_)),
            InputState::Ghost(ghost) => matches!(ghost.source, GhostSource::Const(_)),
            InputState::Unbound => false,
        };
        EditFacts {
            binding_kind: constant(&self.state.from) != constant(&self.state.to),
            ..EditFacts::NONE
        }
    }
}
