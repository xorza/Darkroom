use serde::de;

use super::error::{Result, ScnError};
use super::value::ScnValue;

// ===========================================================================
// Deserializer: ScnValue → T
// ===========================================================================

impl<'de> de::Deserializer<'de> for ScnValue {
    type Error = ScnError;

    fn deserialize_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            ScnValue::Null => visitor.visit_unit(),
            ScnValue::Bool(b) => visitor.visit_bool(b),
            ScnValue::Int(i) => visitor.visit_i64(i),
            ScnValue::Uint(u) => visitor.visit_u64(u),
            ScnValue::Float(f) => visitor.visit_f64(f),
            ScnValue::String(s) => visitor.visit_string(s),
            ScnValue::Array(items) => {
                let mut de = SeqDeserializer {
                    iter: items.into_iter(),
                };
                visitor.visit_seq(&mut de)
            }
            ScnValue::Map(entries) => {
                let mut de = MapDeserializer {
                    iter: entries.into_iter(),
                    current_value: None,
                };
                visitor.visit_map(&mut de)
            }
            ScnValue::Variant(tag, None) => visitor.visit_string(tag),
            ScnValue::Variant(tag, Some(payload)) => {
                // For deserialize_any, fall back to single-key map representation
                let mut de = MapDeserializer {
                    iter: vec![(tag, *payload)].into_iter(),
                    current_value: None,
                };
                visitor.visit_map(&mut de)
            }
        }
    }

    fn deserialize_bool<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            ScnValue::Bool(b) => visitor.visit_bool(b),
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_i8<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_i64(visitor)
    }
    fn deserialize_i16<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_i64(visitor)
    }
    fn deserialize_i32<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_i64(visitor)
    }
    fn deserialize_i64<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            ScnValue::Int(i) => visitor.visit_i64(i),
            ScnValue::Uint(u) => visitor.visit_u64(u),
            ScnValue::Float(f) => visitor.visit_f64(f),
            _ => self.deserialize_any(visitor),
        }
    }
    fn deserialize_u8<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_u64(visitor)
    }
    fn deserialize_u16<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_u64(visitor)
    }
    fn deserialize_u32<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_u64(visitor)
    }
    fn deserialize_u64<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            ScnValue::Uint(u) => visitor.visit_u64(u),
            ScnValue::Int(i) => visitor.visit_i64(i),
            ScnValue::Float(f) => visitor.visit_f64(f),
            _ => self.deserialize_any(visitor),
        }
    }
    fn deserialize_f32<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_f64(visitor)
    }
    fn deserialize_f64<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            ScnValue::Float(f) => visitor.visit_f64(f),
            ScnValue::Int(i) => visitor.visit_f64(i as f64),
            ScnValue::Uint(u) => visitor.visit_f64(u as f64),
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_char<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_str(visitor)
    }

    fn deserialize_str<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            ScnValue::String(s) => visitor.visit_string(s),
            ScnValue::Int(i) => visitor.visit_string(i.to_string()),
            ScnValue::Uint(u) => visitor.visit_string(u.to_string()),
            ScnValue::Float(f) => visitor.visit_string(f.to_string()),
            ScnValue::Bool(b) => visitor.visit_string(b.to_string()),
            ScnValue::Null => visitor.visit_string("null".to_string()),
            ScnValue::Variant(tag, None) => visitor.visit_string(tag),
            _ => Err(ScnError::Message(format!(
                "expected string, got {:?}",
                self
            ))),
        }
    }

    fn deserialize_string<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: de::Visitor<'de>>(self, _visitor: V) -> Result<V::Value> {
        Err(ScnError::Message("byte arrays not supported".to_string()))
    }
    fn deserialize_byte_buf<V: de::Visitor<'de>>(self, _visitor: V) -> Result<V::Value> {
        Err(ScnError::Message("byte arrays not supported".to_string()))
    }

    fn deserialize_option<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            ScnValue::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            ScnValue::Null => visitor.visit_unit(),
            _ => Err(ScnError::Message(format!("expected null, got {:?}", self))),
        }
    }

    fn deserialize_unit_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            ScnValue::Array(items) => {
                let mut de = SeqDeserializer {
                    iter: items.into_iter(),
                };
                visitor.visit_seq(&mut de)
            }
            _ => Err(ScnError::Message(format!("expected array, got {:?}", self))),
        }
    }

    fn deserialize_tuple<V: de::Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self {
            ScnValue::Map(entries) => {
                let mut de = MapDeserializer {
                    iter: entries.into_iter(),
                    current_value: None,
                };
                visitor.visit_map(&mut de)
            }
            _ => Err(ScnError::Message(format!("expected map, got {:?}", self))),
        }
    }

    fn deserialize_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        match self {
            // Unit variant from string
            ScnValue::String(s) => visitor.visit_enum(EnumDeserializer {
                variant: s,
                value: None,
            }),
            // Variant node
            ScnValue::Variant(tag, payload) => visitor.visit_enum(EnumDeserializer {
                variant: tag,
                value: payload.map(|v| *v),
            }),
            // Fallback: single-key map (for compatibility with JSON-style)
            ScnValue::Map(mut entries) if entries.len() == 1 => {
                let (variant, value) = entries.remove(0);
                visitor.visit_enum(EnumDeserializer {
                    variant,
                    value: Some(value),
                })
            }
            _ => Err(ScnError::Message(format!(
                "expected string, variant, or single-key map for enum, got {:?}",
                self
            ))),
        }
    }

    fn deserialize_identifier<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_unit()
    }
}

// ---------------------------------------------------------------------------
// Sequence deserializer
// ---------------------------------------------------------------------------

struct SeqDeserializer {
    iter: std::vec::IntoIter<ScnValue>,
}

impl<'de> de::SeqAccess<'de> for SeqDeserializer {
    type Error = ScnError;
    fn next_element_seed<T: de::DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>> {
        match self.iter.next() {
            Some(value) => seed.deserialize(value).map(Some),
            None => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Map deserializer
// ---------------------------------------------------------------------------

struct MapDeserializer {
    iter: std::vec::IntoIter<(String, ScnValue)>,
    current_value: Option<ScnValue>,
}

impl<'de> de::MapAccess<'de> for MapDeserializer {
    type Error = ScnError;

    fn next_key_seed<K: de::DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        match self.iter.next() {
            Some((key, value)) => {
                self.current_value = Some(value);
                seed.deserialize(ScnValue::String(key)).map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: de::DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        let value = self
            .current_value
            .take()
            .expect("next_value_seed called before next_key_seed");
        seed.deserialize(value)
    }
}

// ---------------------------------------------------------------------------
// Enum deserializer
// ---------------------------------------------------------------------------

struct EnumDeserializer {
    variant: String,
    value: Option<ScnValue>,
}

impl<'de> de::EnumAccess<'de> for EnumDeserializer {
    type Error = ScnError;
    type Variant = VariantDeserializer;

    fn variant_seed<V: de::DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant)> {
        let variant = seed.deserialize(ScnValue::String(self.variant))?;
        Ok((variant, VariantDeserializer { value: self.value }))
    }
}

struct VariantDeserializer {
    value: Option<ScnValue>,
}

impl<'de> de::VariantAccess<'de> for VariantDeserializer {
    type Error = ScnError;

    fn unit_variant(self) -> Result<()> {
        if self.value.is_some() {
            return Err(ScnError::Message(
                "expected unit variant, got value".to_string(),
            ));
        }
        Ok(())
    }

    fn newtype_variant_seed<T: de::DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        match self.value {
            Some(value) => seed.deserialize(value),
            None => Err(ScnError::Message(
                "expected newtype variant value".to_string(),
            )),
        }
    }

    fn tuple_variant<V: de::Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value> {
        match self.value {
            Some(ScnValue::Array(items)) => {
                let mut de = SeqDeserializer {
                    iter: items.into_iter(),
                };
                visitor.visit_seq(&mut de)
            }
            Some(other) => Err(ScnError::Message(format!(
                "expected array for tuple variant, got {:?}",
                other
            ))),
            None => Err(ScnError::Message(
                "expected tuple variant value".to_string(),
            )),
        }
    }

    fn struct_variant<V: de::Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        match self.value {
            Some(ScnValue::Map(entries)) => {
                let mut de = MapDeserializer {
                    iter: entries.into_iter(),
                    current_value: None,
                };
                visitor.visit_map(&mut de)
            }
            Some(other) => Err(ScnError::Message(format!(
                "expected map for struct variant, got {:?}",
                other
            ))),
            None => Err(ScnError::Message(
                "expected struct variant value".to_string(),
            )),
        }
    }
}
