use ::serde::de::Error as SerdeError;
use ::serde::{Deserialize, Deserializer, Serializer};
use glam::Vec2;
use indexmap::IndexMap;
use scenarium::NodeId;

pub(super) fn serialize<S>(
    placements: &IndexMap<NodeId, Vec2>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.collect_seq(placements.iter())
}

pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<IndexMap<NodeId, Vec2>, D::Error>
where
    D: Deserializer<'de>,
{
    let entries = Vec::<(NodeId, Vec2)>::deserialize(deserializer)?;
    let mut placements = IndexMap::with_capacity(entries.len());
    for (key, position) in entries {
        if placements.insert(key, position).is_some() {
            return Err(SerdeError::custom("duplicate graph-view item"));
        }
    }
    Ok(placements)
}

#[cfg(test)]
mod tests;
