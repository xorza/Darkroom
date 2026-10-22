//! [`SharedGains`]: the source gains a run's frames share, each held once.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::image::flat_gain::FlatGain;
use crate::image::flat_gain::gain_key::GainKey;

/// The source gains a run's frames share, each held once for the run: a decode whose gain is one
/// the run holds takes that one and drops its own, or reads none when its file states the key. Held
/// for the whole run, so a frame decoded after every frame of its gain is gone reads none again
/// either.
#[derive(Debug, Default)]
pub(crate) struct SharedGains {
    held: Mutex<Vec<Arc<FlatGain>>>,
}

impl SharedGains {
    /// The gain of `key`, when the run holds it.
    pub(crate) fn get(&self, key: GainKey) -> Option<Arc<FlatGain>> {
        self.lock()
            .iter()
            .find(|gain| gain.key() == key)
            .map(Arc::clone)
    }

    /// `gain` as the run's frames share it: the one the run holds of its key, or `gain`, held from
    /// now on.
    pub(crate) fn share(&self, gain: FlatGain) -> Arc<FlatGain> {
        let key = gain.key();
        let mut held = self.lock();
        if let Some(shared) = held.iter().find(|held| held.key() == key) {
            return Arc::clone(shared);
        }
        let shared = Arc::new(gain);
        held.push(Arc::clone(&shared));
        shared
    }

    /// The gains, poisoned or not: every critical section is a find or a push.
    fn lock(&self) -> MutexGuard<'_, Vec<Arc<FlatGain>>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
