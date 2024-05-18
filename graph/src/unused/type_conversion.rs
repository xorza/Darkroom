use std::borrow::Borrow;
use std::hash::{Hash, Hasher};

use crate::data::DataType;

trait TypeConverterKey {
    // -> (src, dst)
    fn key(&self) -> (&DataType, &DataType);
}

#[derive(Debug, Clone)]
pub struct TypeConverterDesc {
    pub src: DataType,
    pub dst: DataType,
}


impl TypeConverterKey for TypeConverterDesc {
    fn key(&self) -> (&DataType, &DataType) {
        (&self.src, &self.dst)
    }
}
impl<'a> TypeConverterKey for (&'a DataType, &'a DataType) {
    fn key(&self) -> (&DataType, &DataType) {
        (self.0, self.1)
    }
}

impl Hash for dyn TypeConverterKey + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}
impl PartialEq for dyn TypeConverterKey + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}
impl Eq for dyn TypeConverterKey + '_ {}

impl Hash for TypeConverterDesc {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}
impl PartialEq for TypeConverterDesc {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}
impl Eq for TypeConverterDesc {}

impl<'a> Borrow<dyn TypeConverterKey + 'a> for TypeConverterDesc {
    fn borrow(&self) -> &(dyn TypeConverterKey + 'a) {
        self
    }
}
impl<'a> Borrow<dyn TypeConverterKey + 'a> for (&'a DataType, &'a DataType) {
    fn borrow(&self) -> &(dyn TypeConverterKey + 'a) {
        self
    }
}


#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn test_type_converter_desc_hash() {
        let mut map = HashMap::new();
        map.insert(
            TypeConverterDesc { src: DataType::Int, dst: DataType::Int },
            13,
        );

        let borrowed = (&DataType::Int, &DataType::Int);

        assert_eq!(map.get(&borrowed as &dyn TypeConverterKey), Some(&13));
    }
}

