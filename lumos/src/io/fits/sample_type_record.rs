//! [`SampleTypeRecord`]: a FITS sample type as a kept frame's record holds it.

use fits_well::image::SampleType;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// [`SampleType`] as a record holds it: `fits-well` stays free of serde, so the type is mirrored
/// here, variant for variant, and converted both ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SampleTypeRecord {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
}

impl SampleTypeRecord {
    /// Serialize an optional sample type through its record: what
    /// [`ImageMetadata::sample_type`](crate::ImageMetadata::sample_type) is written as.
    #[expect(
        clippy::ref_option,
        clippy::trivially_copy_pass_by_ref,
        reason = "serde's `serialize_with` hands the field over by reference"
    )]
    pub(crate) fn serialize_option<S: Serializer>(
        value: &Option<SampleType>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.map(Self::from).serialize(serializer)
    }

    /// Read back what [`Self::serialize_option`] wrote.
    pub(crate) fn deserialize_option<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<SampleType>, D::Error> {
        Ok(Option::<Self>::deserialize(deserializer)?.map(SampleType::from))
    }
}

impl From<SampleType> for SampleTypeRecord {
    fn from(value: SampleType) -> Self {
        match value {
            SampleType::I8 => Self::I8,
            SampleType::U8 => Self::U8,
            SampleType::I16 => Self::I16,
            SampleType::U16 => Self::U16,
            SampleType::I32 => Self::I32,
            SampleType::U32 => Self::U32,
            SampleType::I64 => Self::I64,
            SampleType::U64 => Self::U64,
            SampleType::F32 => Self::F32,
            SampleType::F64 => Self::F64,
        }
    }
}

impl From<SampleTypeRecord> for SampleType {
    fn from(value: SampleTypeRecord) -> Self {
        match value {
            SampleTypeRecord::I8 => Self::I8,
            SampleTypeRecord::U8 => Self::U8,
            SampleTypeRecord::I16 => Self::I16,
            SampleTypeRecord::U16 => Self::U16,
            SampleTypeRecord::I32 => Self::I32,
            SampleTypeRecord::U32 => Self::U32,
            SampleTypeRecord::I64 => Self::I64,
            SampleTypeRecord::U64 => Self::U64,
            SampleTypeRecord::F32 => Self::F32,
            SampleTypeRecord::F64 => Self::F64,
        }
    }
}
