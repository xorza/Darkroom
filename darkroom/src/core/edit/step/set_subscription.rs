//! Adding and removing one event-subscription edge.

use scenarium::{GraphId, Subscription, SubscriptionState};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;
use crate::core::edit::step::set_ties::SetTies;

/// Whether one `subscriber ← emitter.event` edge is made, kept as a ghost, or neither, before
/// and after.
///
/// Subscribing and unsubscribing are exact inverses, so one step backs both. An edit never
/// makes a ghost; one that subscribes or unsubscribes takes the ghost of the edge, and the undo
/// puts it back.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetSubscription {
    pub(crate) subscription: Subscription,
    pub(crate) state: Change<SubscriptionState>,
    /// The route of the event wire an unsubscribe ends, and each tie that
    /// only it held.
    pub(crate) ties: SetTies,
}

impl Reversible for SetSubscription {
    fn write(&self, document: &mut Document, graph: GraphId, dir: Direction) {
        document
            .slot_mut(graph)
            .graph
            .set_subscription_state(self.subscription, self.state.half(dir).clone());
        self.ties.write(document, graph, dir);
    }

    /// Dropping an event wire on a pin that already carries it lands here.
    /// The ties change only with the subscription.
    fn is_noop(&self) -> bool {
        self.state.unchanged()
    }

    fn dirties_document(&self) -> bool {
        true
    }

    fn facts(&self) -> EditFacts {
        EditFacts::NONE
    }
}
