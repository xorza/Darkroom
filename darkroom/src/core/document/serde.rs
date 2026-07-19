use ::serde::de::Error as SerdeError;
use ::serde::{Deserialize, Deserializer, Serializer};
use glam::Vec2;
use indexmap::IndexMap;

use crate::core::document::ItemRef;

pub(crate) fn serialize<S>(
    placements: &IndexMap<ItemRef, Vec2>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.collect_seq(placements.iter())
}

pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<IndexMap<ItemRef, Vec2>, D::Error>
where
    D: Deserializer<'de>,
{
    let entries = Vec::<(ItemRef, Vec2)>::deserialize(deserializer)?;
    let mut placements = IndexMap::with_capacity(entries.len());
    for (key, position) in entries {
        if placements.insert(key, position).is_some() {
            return Err(SerdeError::custom("duplicate graph-view item"));
        }
    }
    Ok(placements)
}

#[cfg(test)]
mod tests {
    use ::serde::{Deserialize, Serialize};
    use indexmap::IndexMap;
    use scenarium::NodeId;

    use crate::core::document::ItemRef;
    use glam::Vec2;

    #[derive(Debug, Serialize, Deserialize)]
    struct Fixture {
        #[serde(with = "crate::core::document::serde")]
        placements: IndexMap<ItemRef, Vec2>,
    }

    #[test]
    fn duplicate_item_keys_are_rejected() {
        let mut placements = IndexMap::new();
        placements.insert(ItemRef::Node(NodeId::unique()), Vec2::new(1.0, 2.0));
        let mut encoded = serde_json::to_value(Fixture { placements }).unwrap();
        let entries = encoded["placements"].as_array_mut().unwrap();
        let duplicate = entries[0].clone();
        entries.push(duplicate);

        let error = serde_json::from_value::<Fixture>(encoded).unwrap_err();
        assert!(error.to_string().contains("duplicate graph-view item"));
    }
}
