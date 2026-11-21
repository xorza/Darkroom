//! [`FuncSignature`]: a digest of a func's ports, which a compiled node's state and cached values
//! are owned by, and the [`FuncRef`] a compiled node runs under.

use std::fmt;

use crate::data::data_type::{DataType, FileMode};
use crate::decl::ports::{EventDecl, Numbered, Ports};
use crate::identity::FuncId;

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
    /// [`FuncBuilder::checked`](crate::decl::func::func_builder::FuncBuilder::checked)
    /// seals into a [`Func`](crate::decl::func::Func).
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

/// The implementation a compiled node runs: its func's id and the signature of its ports, which
/// a node's state and cached values are owned by. Internal to the crate: a node names its func by
/// its [`FuncId`] alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FuncRef {
    id: FuncId,
    signature: FuncSignature,
}

impl FuncRef {
    pub(crate) const fn new(id: FuncId, signature: FuncSignature) -> Self {
        Self { id, signature }
    }

    pub(crate) const fn id(self) -> FuncId {
        self.id
    }

    pub(crate) const fn signature(self) -> FuncSignature {
        self.signature
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
mod tests;
