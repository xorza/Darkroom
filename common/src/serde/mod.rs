pub mod hex_bytes;
pub(crate) mod serde_format;

use ron::de::SpannedError;

#[derive(Debug, thiserror::Error)]
pub enum SerializeError {
    #[error("RON serialization failed")]
    Ron(#[from] ron::Error),
    #[error("Bitcode serialization failed")]
    Bitcode(#[from] bitcode::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum DeserializeError {
    #[error("RON deserialization failed")]
    Ron(#[from] SpannedError),
    #[error("Bitcode deserialization failed")]
    Bitcode(#[from] bitcode::Error),
}

#[cfg(test)]
mod tests;
