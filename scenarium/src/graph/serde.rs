use std::collections::BTreeMap;

use ::serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::graph::{Binding, InputPort};

/// Struct keys cannot be map keys in string-keyed formats such as JSON and TOML.
pub(crate) fn serialize<S: Serializer>(
    map: &BTreeMap<InputPort, Binding>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    map.iter().collect::<Vec<_>>().serialize(serializer)
}

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<InputPort, Binding>, D::Error> {
    Ok(Vec::<(InputPort, Binding)>::deserialize(deserializer)?
        .into_iter()
        .collect())
}
