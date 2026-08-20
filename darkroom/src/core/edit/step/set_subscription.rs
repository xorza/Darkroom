//! Adding and removing one event-subscription edge.

use scenarium::Subscription;
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;

/// Whether one `subscriber ← emitter.event` edge is wired, before and after.
///
/// Subscribing and unsubscribing are exact inverses, so one step backs both:
/// each half is simply the subscribed-state boolean, and a write means
/// "make it so".
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetSubscription {
    pub(crate) subscription: Subscription,
    pub(crate) subscribed: Change<bool>,
}

impl Reversible for SetSubscription {
    fn write(&self, doc: &mut Document, dir: Direction) {
        let Subscription {
            emitter,
            event_idx,
            subscriber,
        } = self.subscription;
        if *self.subscribed.half(dir) {
            doc.graph.subscribe(emitter, event_idx, subscriber);
        } else {
            doc.graph.unsubscribe(emitter, event_idx, subscriber);
        }
    }

    /// Dropping an event wire on a pin that already carries it lands here.
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
}
