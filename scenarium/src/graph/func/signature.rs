//! A func's port signature: what a document's bindings and subscriptions are indexed against.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::DataType;
use crate::graph::func::{Func, OutputType};

/// A digest of a func's ports: its inputs' names and types, its outputs' names and types, and
/// its events' names, in order.
///
/// Bindings and subscriptions name ports by index, so a func whose ports moved or changed type
/// under a saved node would bind that node's wiring to the wrong ports, or silently drop it. A
/// node records the signature of the func it was authored against, and a mismatch with the
/// library is refused by name rather than lowered into a wrong graph.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FuncSignature([u8; 32]);

impl FuncSignature {
    /// Versions the encoding below: a change to it is a change to every signature.
    const DOMAIN: &'static [u8] = b"scenarium-func-signature-v1";

    pub fn of(func: &Func) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(Self::DOMAIN);
        let count = |hasher: &mut blake3::Hasher, count: usize| {
            hasher.update(&(count as u64).to_le_bytes());
        };
        let name = |hasher: &mut blake3::Hasher, name: &str| {
            hasher.update(&(name.len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
        };
        count(&mut hasher, func.inputs.len());
        for input in &func.inputs {
            name(&mut hasher, &input.name);
            Self::write_type(&mut hasher, &input.data_type);
        }
        count(&mut hasher, func.outputs.len());
        for output in &func.outputs {
            name(&mut hasher, &output.name);
            match &output.ty {
                OutputType::Fixed(data_type) => {
                    hasher.update(&[0]);
                    Self::write_type(&mut hasher, data_type);
                }
                OutputType::Wildcard { mirrors } => {
                    hasher.update(&[1]);
                    hasher.update(&(*mirrors as u64).to_le_bytes());
                }
            }
        }
        count(&mut hasher, func.events.len());
        for event in &func.events {
            name(&mut hasher, &event.name);
        }
        Self(*hasher.finalize().as_bytes())
    }

    /// A type as its kind, plus the identity of a custom or enum type. A path's mode and
    /// extensions are left out: they decide which values a picker offers, not where a binding
    /// lands.
    fn write_type(hasher: &mut blake3::Hasher, data_type: &DataType) {
        let tag: u8 = match data_type {
            DataType::Any => 0,
            DataType::Float => 1,
            DataType::Int => 2,
            DataType::Bool => 3,
            DataType::String => 4,
            DataType::FsPath(_) => 5,
            DataType::Custom(_) => 6,
            DataType::Enum(_) => 7,
        };
        hasher.update(&[tag]);
        if let DataType::Custom(type_id) | DataType::Enum(type_id) = data_type {
            hasher.update(&type_id.as_u128().to_le_bytes());
        }
    }
}

impl fmt::Debug for FuncSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FuncSignature(")?;
        for byte in &self.0[..8] {
            write!(f, "{byte:02x}")?;
        }
        write!(f, "…)")
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use crate::graph::func::signature::FuncSignature;
    use crate::graph::func::{Func, FuncEvent, FuncInput, FuncOutput};
    use crate::graph::identity::FuncId;
    use crate::{DataType, EventLambda};

    /// One small func's signature, from its bytes written out by hand rather than by the
    /// encoder: the domain, then each length-prefixed list — one input `value` of kind 2 (Int),
    /// one fixed output `out` of kind 1 (Float), one event `tick`. The digest is pinned too, so
    /// an encoding that changes on purpose has to bump the domain and this pin together; any
    /// other failure here is drift that would refuse every saved node.
    #[test]
    fn a_small_func_has_the_pinned_signature() {
        let func = Func {
            inputs: vec![FuncInput::required("value", DataType::Int)],
            outputs: vec![FuncOutput::new("out", DataType::Float)],
            events: vec![FuncEvent {
                name: "tick".into(),
                event_lambda: EventLambda::default(),
            }],
            ..Func::new(FuncId::from_u128(1), "pinned")
        };
        let count = |n: u64| n.to_le_bytes().to_vec();
        let name = |text: &str| [count(text.len() as u64), text.as_bytes().to_vec()].concat();
        let layout = [
            b"scenarium-func-signature-v1".to_vec(),
            count(1),
            name("value"),
            vec![2],
            count(1),
            name("out"),
            vec![0, 1],
            count(1),
            name("tick"),
        ]
        .concat();
        let signature = FuncSignature::of(&func);
        assert_eq!(signature.0, *blake3::hash(&layout).as_bytes());
        let hex = signature.0.iter().fold(String::new(), |mut hex, byte| {
            write!(hex, "{byte:02x}").unwrap();
            hex
        });
        assert_eq!(
            hex,
            "308a00dec0dd99b316897f7b18a471352cc0d9e03f4e607f70526b681700c4bc"
        );
    }
}
