//! Adding and removing one event-subscription edge.

use scenarium::Subscription;
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;
use crate::core::edit::step::set_ties::SetTies;

/// Whether one `subscriber ← emitter.event` edge is wired, before and after.
///
/// Subscribing and unsubscribing are exact inverses, so one step backs both:
/// each half is simply the subscribed-state boolean, and a write means
/// "make it so".
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetSubscription {
    pub(crate) subscription: Subscription,
    pub(crate) subscribed: Change<bool>,
    /// The route of the event wire an unsubscribe ends, and each tie that
    /// only it held.
    pub(crate) ties: SetTies,
}

impl Reversible for SetSubscription {
    fn write(&self, document: &mut Document, dir: Direction) {
        let Subscription { event, subscriber } = self.subscription;
        if *self.subscribed.half(dir) {
            document.graph.subscribe(event, subscriber);
        } else {
            document.graph.unsubscribe(event, subscriber);
        }
        self.ties.write(document, dir);
    }

    /// Dropping an event wire on a pin that already carries it lands here.
    /// The ties change only with the subscription.
    fn is_noop(&self) -> bool {
        self.subscribed.unchanged()
    }

    fn dirties_document(&self) -> bool {
        true
    }

    /// An event wire paints between glyphs that are already there; no node
    /// remeasures.
    fn invalidates_cached_geometry(&self) -> bool {
        false
    }

    fn changes_node_set(&self) -> bool {
        false
    }
}
