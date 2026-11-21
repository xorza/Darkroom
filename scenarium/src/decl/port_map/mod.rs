//! [`PortMap`] and [`NodeRemap`]: where each port of one declaration went in another.
//!
//! # Decisions
//!
//! - **One remap moves a node from one declaration to another**, for a func a person replaces
//!   and for a func the library redirects: three maps, inputs, outputs and events, each a partial
//!   injection from old port ids to new ones. Nothing pairs by name here: a name is a guess a
//!   person confirms before a remap is made, never one a load acts on.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::decl::saved_decl::SavedDecl;
use crate::identity::PortId;

/// Old port ids to new ones, each side at most once. Built by [`Self::checked`] alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<(PortId, PortId)>", into = "Vec<(PortId, PortId)>")]
pub struct PortMap {
    /// Sorted by old id.
    pairs: Vec<(PortId, PortId)>,
}

/// Why [`PortMap::checked`] refuses a map.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum PortMapError {
    #[error("old port {0} maps twice")]
    OldTwice(PortId),
    #[error("new port {0} is the target of two old ports")]
    NewTwice(PortId),
}

impl PortMap {
    /// The map of `pairs`, old to new; refused where an old or a new id appears twice.
    pub fn checked(
        pairs: impl IntoIterator<Item = (PortId, PortId)>,
    ) -> Result<Self, PortMapError> {
        let mut pairs: Vec<(PortId, PortId)> = pairs.into_iter().collect();
        pairs.sort_unstable();
        if let Some(pair) = pairs.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            return Err(PortMapError::OldTwice(pair[0].0));
        }
        let mut targets: Vec<PortId> = pairs.iter().map(|(_, new)| *new).collect();
        targets.sort_unstable();
        if let Some(pair) = targets.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(PortMapError::NewTwice(pair[0]));
        }
        Ok(Self { pairs })
    }

    /// [`Self::checked`], panicking on its error.
    pub fn new(pairs: impl IntoIterator<Item = (PortId, PortId)>) -> Self {
        Self::checked(pairs).expect("a port map is an injection")
    }

    /// Where old port `old` went, if the map names it.
    pub fn get(&self, old: PortId) -> Option<PortId> {
        self.pairs
            .binary_search_by_key(&old, |(from, _)| *from)
            .ok()
            .map(|idx| self.pairs[idx].1)
    }

    /// In old-id order.
    pub fn pairs(&self) -> &[(PortId, PortId)] {
        &self.pairs
    }

    /// The new ports the map moves a port to, in old-id order.
    pub(crate) fn targets(&self) -> impl Iterator<Item = PortId> + '_ {
        self.pairs.iter().map(|(_, new)| *new)
    }

    /// This map followed by `next`: each old port to where `next` takes the port this map takes
    /// it to, dropped where `next` names none.
    #[must_use]
    pub fn then(&self, next: &Self) -> Self {
        Self::new(
            self.pairs
                .iter()
                .filter_map(|&(old, mid)| Some((old, next.get(mid)?))),
        )
    }

    /// The pairs a person would expect from `before` to `after`: a port to the new one of the
    /// same id and type, else to an unpaired one of the same name and type. `same` says whether
    /// two ports agree in type.
    fn propose(
        before: &[(PortId, &str)],
        after: &[(PortId, &str)],
        same: impl Fn(PortId, PortId) -> bool,
    ) -> Self {
        let mut pairs: Vec<(PortId, PortId)> = Vec::new();
        let taken =
            |pairs: &[(PortId, PortId)], new: PortId| pairs.iter().any(|(_, to)| *to == new);
        for &(old, _) in before {
            if after.iter().any(|(new, _)| *new == old) && same(old, old) {
                pairs.push((old, old));
            }
        }
        for &(old, name) in before {
            if pairs.iter().any(|(from, _)| *from == old) {
                continue;
            }
            if let Some(&(new, _)) = after
                .iter()
                .find(|&&(new, to)| to == name && same(old, new) && !taken(&pairs, new))
            {
                pairs.push((old, new));
            }
        }
        Self::new(pairs)
    }
}

impl TryFrom<Vec<(PortId, PortId)>> for PortMap {
    type Error = PortMapError;

    fn try_from(pairs: Vec<(PortId, PortId)>) -> Result<Self, PortMapError> {
        Self::checked(pairs)
    }
}

impl From<PortMap> for Vec<(PortId, PortId)> {
    fn from(map: PortMap) -> Self {
        map.pairs
    }
}

/// Where each port of a node's old declaration went in its new one: its inputs, its outputs and
/// its events.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeRemap {
    pub inputs: PortMap,
    pub outputs: PortMap,
    pub events: PortMap,
}

impl NodeRemap {
    /// The remap a person would expect from the declaration `before` to `after`: each port to
    /// the new one of its id and type, else to an unpaired one of its name and type; an event, which
    /// has no type, by its id, else its name. A proposal a person confirms before it is made.
    pub fn propose(before: &SavedDecl, after: &SavedDecl) -> Self {
        let [inputs_before, inputs_after] = [before, after].map(|decl| {
            decl.inputs
                .iter()
                .map(|input| (input.id, input.name.as_str()))
                .collect::<Vec<_>>()
        });
        let [outputs_before, outputs_after] = [before, after].map(|decl| {
            decl.outputs
                .iter()
                .map(|output| (output.id, output.name.as_str()))
                .collect::<Vec<_>>()
        });
        let [events_before, events_after] = [before, after].map(|decl| {
            decl.events
                .iter()
                .map(|event| (event.id, event.name.as_str()))
                .collect::<Vec<_>>()
        });
        let same_input = |old: PortId, new: PortId| {
            before.input(old).map(|input| input.data_type)
                == after.input(new).map(|input| input.data_type)
        };
        let same_output = |old: PortId, new: PortId| {
            before.output(old).map(|output| output.data_type)
                == after.output(new).map(|output| output.data_type)
        };
        Self {
            inputs: PortMap::propose(&inputs_before, &inputs_after, same_input),
            outputs: PortMap::propose(&outputs_before, &outputs_after, same_output),
            events: PortMap::propose(&events_before, &events_after, |_, _| true),
        }
    }

    /// This remap followed by `next`, side by side.
    #[must_use]
    pub fn then(&self, next: &Self) -> Self {
        Self {
            inputs: self.inputs.then(&next.inputs),
            outputs: self.outputs.then(&next.outputs),
            events: self.events.then(&next.events),
        }
    }
}

#[cfg(test)]
mod tests;
