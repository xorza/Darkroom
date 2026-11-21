//! [`FuncSignature`]: a digest of a func's ports, which a compiled node's state and cached values
//! are owned by.

use std::fmt;

use crate::data::data_type::{DataType, FileMode};
use crate::graph::ports::{EventDecl, Numbered, Ports};

/// A digest of a func's ports: each input's and output's id and type, and each event's id, in
/// order.
///
/// A saved node names its func by id alone and its wiring names ports by id, so no document holds
/// this. It keys what a run keeps of a node across installs and processes: its cached values and
/// its state. A func whose ports changed, a port added, moved or retyped, keys none of what the
/// previous ports produced. A port's name decides nothing a run reads, so a rename keeps the key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FuncSignature([u8; 32]);

impl FuncSignature {
    /// Versions the encoding below: a change to it is a change to every signature.
    const DOMAIN: &'static [u8] = b"scenarium-func-signature-v4";

    /// The signature of `ports` and `events`, in order — what
    /// [`FuncBuilder::checked`](crate::graph::func::func_builder::FuncBuilder::checked)
    /// seals into a [`Func`](crate::graph::func::Func).
    pub(crate) fn of(ports: &Ports, events: &[Numbered<EventDecl>]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(Self::DOMAIN);
        let count = |hasher: &mut blake3::Hasher, count: usize| {
            hasher.update(&(count as u64).to_le_bytes());
        };
        count(&mut hasher, ports.inputs.len());
        for input in &ports.inputs {
            hasher.update(&input.id.get().to_le_bytes());
            write_type(&mut hasher, input.data_type);
        }
        count(&mut hasher, ports.outputs.len());
        for output in &ports.outputs {
            hasher.update(&output.id.get().to_le_bytes());
            write_type(&mut hasher, output.data_type);
        }
        count(&mut hasher, events.len());
        for event in events {
            hasher.update(&event.id.get().to_le_bytes());
        }
        Self(*hasher.finalize().as_bytes())
    }

    /// The digest's bytes, for a fold that keys on the signature.
    pub(crate) const fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// A type as its kind, plus the identity of what it names in the library: a cached value of one
/// custom type, enum or file kind is no value of another.
fn write_type(hasher: &mut blake3::Hasher, data_type: DataType) {
    let (tag, id): (u8, u128) = match data_type {
        DataType::Any => (0, 0),
        DataType::Float => (1, 0),
        DataType::Int => (2, 0),
        DataType::Bool => (3, 0),
        DataType::String => (4, 0),
        DataType::Dir => (5, 0),
        DataType::Custom(type_id) => (6, type_id.as_u128()),
        DataType::Enum(enum_id) => (7, enum_id.as_u128()),
        DataType::File {
            kind,
            mode: FileMode::Open,
        } => (8, kind.as_u128()),
        DataType::File {
            kind,
            mode: FileMode::OpenMany,
        } => (9, kind.as_u128()),
        DataType::File {
            kind,
            mode: FileMode::Save,
        } => (10, kind.as_u128()),
    };
    hasher.update(&[tag]);
    hasher.update(&id.to_le_bytes());
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
    use crate::data::data_type::file_kind::FileKindId;
    use crate::data::data_type::{DataType, FileMode};
    use crate::graph::func::signature::FuncSignature;
    use crate::graph::identity::FuncId;
    use crate::graph::ports::{EventDecl, InputDecl, OutputDecl};
    use crate::internals;

    /// One small func's signature, from its bytes written out by hand rather than by the
    /// encoder: the domain, then each length-prefixed list — input 3 of kind 2 (Int), output 1 of
    /// kind 1 (Float), event 2 — each port id 4 bytes and each type its tag and 16 id bytes.
    #[test]
    fn a_small_func_has_the_pinned_signature() {
        let func = internals::stub_func(FuncId::from_u128(1), "pinned")
            .input(3, InputDecl::required("value", DataType::Int))
            .output(1, OutputDecl::new("out", DataType::Float))
            .event(2, EventDecl::new("tick"), internals::stub_event())
            .build();
        let count = |n: u64| n.to_le_bytes().to_vec();
        let port = |n: u32| n.to_le_bytes().to_vec();
        let type_id = |n: u128| n.to_le_bytes().to_vec();
        let layout = [
            b"scenarium-func-signature-v4".to_vec(),
            count(1),
            port(3),
            vec![2],
            type_id(0),
            count(1),
            port(1),
            vec![1],
            type_id(0),
            count(1),
            port(2),
        ]
        .concat();
        assert_eq!(
            func.func_ref().signature(),
            FuncSignature(*blake3::hash(&layout).as_bytes())
        );
    }

    /// The signature changes with what a run reads of a port, its id and its type, a file's
    /// kind and mode included, and not with its name or description.
    #[test]
    fn a_signature_covers_ids_and_types_and_not_names() {
        let file = |kind, mode| DataType::File {
            kind: FileKindId::from_u128(kind),
            mode,
        };
        let signature = |number: u32, input: InputDecl| {
            internals::stub_func(FuncId::from_u128(1), "f")
                .input(number, input)
                .build()
                .func_ref()
                .signature()
        };
        let base = signature(1, InputDecl::required("a", file(1, FileMode::Open)));
        assert_eq!(
            signature(
                1,
                InputDecl::optional("renamed", file(1, FileMode::Open)).description("text")
            ),
            base
        );
        for changed in [
            signature(2, InputDecl::required("a", file(1, FileMode::Open))),
            signature(1, InputDecl::required("a", file(2, FileMode::Open))),
            signature(1, InputDecl::required("a", file(1, FileMode::Save))),
            signature(1, InputDecl::required("a", DataType::Dir)),
        ] {
            assert_ne!(changed, base);
        }
    }
}
