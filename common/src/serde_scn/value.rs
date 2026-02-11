use serde::Serialize;

use super::error::{Result, ScnError};

/// Intermediate representation for SCN values.
#[derive(Debug, Clone, PartialEq)]
pub enum ScnValue {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    String(String),
    Array(Vec<ScnValue>),
    Map(Vec<(String, ScnValue)>),
    /// Tagged variant: `Tag`, `Tag value`, `Tag { fields }`.
    /// Maps to serde's externally tagged enum representation.
    Variant(String, Option<Box<ScnValue>>),
}

// ===========================================================================
// Serializer: T → ScnValue (tree-building)
// ===========================================================================

pub struct ValueSerializer;

impl serde::Serializer for ValueSerializer {
    type Ok = ScnValue;
    type Error = ScnError;
    type SerializeSeq = SeqBuilder;
    type SerializeTuple = SeqBuilder;
    type SerializeTupleStruct = SeqBuilder;
    type SerializeTupleVariant = TupleVariantBuilder;
    type SerializeMap = MapBuilder;
    type SerializeStruct = MapBuilder;
    type SerializeStructVariant = StructVariantBuilder;

    fn serialize_bool(self, v: bool) -> Result<ScnValue> {
        Ok(ScnValue::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<ScnValue> {
        Ok(ScnValue::Int(v as i64))
    }
    fn serialize_i16(self, v: i16) -> Result<ScnValue> {
        Ok(ScnValue::Int(v as i64))
    }
    fn serialize_i32(self, v: i32) -> Result<ScnValue> {
        Ok(ScnValue::Int(v as i64))
    }
    fn serialize_i64(self, v: i64) -> Result<ScnValue> {
        Ok(ScnValue::Int(v))
    }
    fn serialize_u8(self, v: u8) -> Result<ScnValue> {
        Ok(ScnValue::Uint(v as u64))
    }
    fn serialize_u16(self, v: u16) -> Result<ScnValue> {
        Ok(ScnValue::Uint(v as u64))
    }
    fn serialize_u32(self, v: u32) -> Result<ScnValue> {
        Ok(ScnValue::Uint(v as u64))
    }
    fn serialize_u64(self, v: u64) -> Result<ScnValue> {
        Ok(ScnValue::Uint(v))
    }
    fn serialize_f32(self, v: f32) -> Result<ScnValue> {
        Ok(ScnValue::Float(v as f64))
    }
    fn serialize_f64(self, v: f64) -> Result<ScnValue> {
        Ok(ScnValue::Float(v))
    }
    fn serialize_char(self, v: char) -> Result<ScnValue> {
        Ok(ScnValue::String(v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<ScnValue> {
        Ok(ScnValue::String(v.to_string()))
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<ScnValue> {
        Err(ScnError::Message("byte arrays not supported".to_string()))
    }
    fn serialize_none(self) -> Result<ScnValue> {
        Ok(ScnValue::Null)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<ScnValue> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<ScnValue> {
        Ok(ScnValue::Null)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<ScnValue> {
        Ok(ScnValue::Null)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<ScnValue> {
        Ok(ScnValue::Variant(variant.to_string(), None))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<ScnValue> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<ScnValue> {
        let inner = value.serialize(ValueSerializer)?;
        Ok(ScnValue::Variant(
            variant.to_string(),
            Some(Box::new(inner)),
        ))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqBuilder> {
        Ok(SeqBuilder {
            items: Vec::with_capacity(len.unwrap_or(0)),
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<SeqBuilder> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<SeqBuilder> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<TupleVariantBuilder> {
        Ok(TupleVariantBuilder {
            variant: variant.to_string(),
            items: Vec::with_capacity(len),
        })
    }
    fn serialize_map(self, len: Option<usize>) -> Result<MapBuilder> {
        Ok(MapBuilder {
            entries: Vec::with_capacity(len.unwrap_or(0)),
            current_key: None,
        })
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<MapBuilder> {
        self.serialize_map(Some(len))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<StructVariantBuilder> {
        Ok(StructVariantBuilder {
            variant: variant.to_string(),
            entries: Vec::with_capacity(len),
        })
    }
}

// ---------------------------------------------------------------------------
// Sequence builder
// ---------------------------------------------------------------------------

pub struct SeqBuilder {
    items: Vec<ScnValue>,
}

impl serde::ser::SerializeSeq for SeqBuilder {
    type Ok = ScnValue;
    type Error = ScnError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        self.items.push(value.serialize(ValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<ScnValue> {
        Ok(ScnValue::Array(self.items))
    }
}

impl serde::ser::SerializeTuple for SeqBuilder {
    type Ok = ScnValue;
    type Error = ScnError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        serde::ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<ScnValue> {
        serde::ser::SerializeSeq::end(self)
    }
}

impl serde::ser::SerializeTupleStruct for SeqBuilder {
    type Ok = ScnValue;
    type Error = ScnError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        serde::ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<ScnValue> {
        serde::ser::SerializeSeq::end(self)
    }
}

// ---------------------------------------------------------------------------
// Tuple variant builder
// ---------------------------------------------------------------------------

pub struct TupleVariantBuilder {
    variant: String,
    items: Vec<ScnValue>,
}

impl serde::ser::SerializeTupleVariant for TupleVariantBuilder {
    type Ok = ScnValue;
    type Error = ScnError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        self.items.push(value.serialize(ValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<ScnValue> {
        Ok(ScnValue::Variant(
            self.variant,
            Some(Box::new(ScnValue::Array(self.items))),
        ))
    }
}

// ---------------------------------------------------------------------------
// Map builder
// ---------------------------------------------------------------------------

pub struct MapBuilder {
    entries: Vec<(String, ScnValue)>,
    current_key: Option<String>,
}

impl serde::ser::SerializeMap for MapBuilder {
    type Ok = ScnValue;
    type Error = ScnError;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<()> {
        let key_value = key.serialize(ValueSerializer)?;
        let key_string = match key_value {
            ScnValue::String(s) => s,
            ScnValue::Int(i) => i.to_string(),
            ScnValue::Uint(u) => u.to_string(),
            ScnValue::Bool(b) => b.to_string(),
            other => {
                return Err(ScnError::Message(format!(
                    "unsupported map key type: {:?}",
                    other
                )));
            }
        };
        self.current_key = Some(key_string);
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        let key = self
            .current_key
            .take()
            .expect("serialize_value called before serialize_key");
        self.entries.push((key, value.serialize(ValueSerializer)?));
        Ok(())
    }
    fn end(self) -> Result<ScnValue> {
        Ok(ScnValue::Map(self.entries))
    }
}

impl serde::ser::SerializeStruct for MapBuilder {
    type Ok = ScnValue;
    type Error = ScnError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.entries
            .push((key.to_string(), value.serialize(ValueSerializer)?));
        Ok(())
    }
    fn end(self) -> Result<ScnValue> {
        Ok(ScnValue::Map(self.entries))
    }
}

// ---------------------------------------------------------------------------
// Struct variant builder
// ---------------------------------------------------------------------------

pub struct StructVariantBuilder {
    variant: String,
    entries: Vec<(String, ScnValue)>,
}

impl serde::ser::SerializeStructVariant for StructVariantBuilder {
    type Ok = ScnValue;
    type Error = ScnError;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.entries
            .push((key.to_string(), value.serialize(ValueSerializer)?));
        Ok(())
    }
    fn end(self) -> Result<ScnValue> {
        Ok(ScnValue::Variant(
            self.variant,
            Some(Box::new(ScnValue::Map(self.entries))),
        ))
    }
}
