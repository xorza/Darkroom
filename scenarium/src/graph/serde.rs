use std::collections::BTreeMap;

use ::serde::de::Error as _;
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
    let entries = Vec::<(InputPort, Binding)>::deserialize(deserializer)?;
    let mut map = BTreeMap::new();
    for (port, binding) in entries {
        if map.insert(port, binding).is_some() {
            return Err(D::Error::custom(format!(
                "duplicate binding for input port {port:?}"
            )));
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize};

    use crate::graph::{Binding, InputPort, NodeId};
    use common::{SerdeFormat, deserialize, serialize};

    #[derive(Debug, Serialize)]
    #[serde(transparent)]
    struct RawBindings(Vec<(InputPort, Binding)>);

    #[derive(Debug)]
    struct CheckedBindings;

    impl<'de> Deserialize<'de> for CheckedBindings {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let _: BTreeMap<InputPort, Binding> = crate::graph::serde::deserialize(deserializer)?;
            Ok(Self)
        }
    }

    #[test]
    fn duplicate_ports_fail_in_every_binding_format() {
        let port = InputPort::new(NodeId::unique(), 0);
        let bindings = RawBindings(vec![
            (port, Binding::Const(1i64.into())),
            (port, Binding::Const(2i64.into())),
        ]);

        for format in [SerdeFormat::Json, SerdeFormat::Bitcode] {
            let bytes = serialize(&bindings, format).unwrap();
            let error = deserialize::<CheckedBindings>(&bytes, format)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("duplicate binding for input port"),
                "unexpected {format:?} error: {error}"
            );
        }
    }
}
