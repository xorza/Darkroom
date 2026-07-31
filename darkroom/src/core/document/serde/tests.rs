use ::serde::{Deserialize, Serialize};
use indexmap::IndexMap;
use scenarium::NodeId;

use glam::Vec2;

#[derive(Debug, Serialize, Deserialize)]
struct Fixture {
    #[serde(with = "crate::core::document::serde")]
    placements: IndexMap<NodeId, Vec2>,
}

#[test]
fn duplicate_item_keys_are_rejected() {
    let mut placements = IndexMap::new();
    placements.insert(NodeId::unique(), Vec2::new(1.0, 2.0));
    let mut encoded = serde_json::to_value(Fixture { placements }).unwrap();
    let entries = encoded["placements"].as_array_mut().unwrap();
    let duplicate = entries[0].clone();
    entries.push(duplicate);

    let error = serde_json::from_value::<Fixture>(encoded).unwrap_err();
    assert!(error.to_string().contains("duplicate graph-view item"));
}
