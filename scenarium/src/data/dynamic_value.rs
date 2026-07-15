use std::any::Any;
use std::fmt::Display;
use std::sync::Arc;

use crate::{StaticValue, TypeId};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RamUsage {
    pub cpu: usize,
    pub gpu: usize,
}

impl RamUsage {
    pub fn total(&self) -> usize {
        self.cpu + self.gpu
    }
}

impl std::ops::Add for RamUsage {
    type Output = RamUsage;

    fn add(self, rhs: RamUsage) -> Self::Output {
        RamUsage {
            cpu: self.cpu + rhs.cpu,
            gpu: self.gpu + rhs.gpu,
        }
    }
}

impl std::ops::AddAssign for RamUsage {
    fn add_assign(&mut self, rhs: RamUsage) {
        self.cpu += rhs.cpu;
        self.gpu += rhs.gpu;
    }
}

pub trait CustomValue: Send + Sync + Display + 'static {
    fn type_id(&self) -> TypeId;
    fn as_any(&self) -> &dyn Any;
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;

    fn ram_bytes(&self) -> RamUsage {
        RamUsage::default()
    }
}

#[derive(Default, Clone)]
pub enum DynamicValue {
    #[default]
    Unbound,
    Static(StaticValue),
    Custom(Arc<dyn CustomValue>),
}

impl std::fmt::Debug for DynamicValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DynamicValue::Unbound => write!(f, "Unbound"),
            DynamicValue::Static(value) => write!(f, "{value:?}"),
            DynamicValue::Custom(data) => f
                .debug_struct("Custom")
                .field("type_id", &data.type_id())
                .finish_non_exhaustive(),
        }
    }
}

impl DynamicValue {
    pub fn from_custom<T: CustomValue>(value: T) -> Self {
        DynamicValue::Custom(Arc::new(value))
    }

    pub fn as_static(&self) -> Option<&StaticValue> {
        match self {
            DynamicValue::Static(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        self.as_static().and_then(StaticValue::as_f64)
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_static().and_then(StaticValue::as_i64)
    }

    pub fn as_bool(&self) -> Option<bool> {
        self.as_static().and_then(StaticValue::as_bool)
    }

    pub fn as_string(&self) -> Option<&str> {
        self.as_static().and_then(StaticValue::as_string)
    }

    pub fn as_enum(&self) -> Option<&str> {
        self.as_static().and_then(StaticValue::as_enum)
    }

    pub fn as_fs_path(&self) -> Option<&str> {
        self.as_static().and_then(StaticValue::as_fs_path)
    }

    pub fn as_custom<T: CustomValue>(&self) -> Option<&T> {
        match self {
            DynamicValue::Custom(data) => data.as_any().downcast_ref::<T>(),
            _ => None,
        }
    }

    pub fn into_custom<T: CustomValue>(self) -> Result<T, Self> {
        let DynamicValue::Custom(data) = self else {
            return Err(self);
        };
        if data.as_any().downcast_ref::<T>().is_none() {
            return Err(DynamicValue::Custom(data));
        }
        let typed = data
            .into_any()
            .downcast::<T>()
            .expect("custom type checked before downcast");
        Arc::try_unwrap(typed).map_err(|shared| DynamicValue::Custom(shared))
    }

    pub fn to_value_string(&self) -> String {
        match self {
            DynamicValue::Unbound => String::new(),
            DynamicValue::Static(value) => value.to_value_string(),
            DynamicValue::Custom(data) => data.to_string(),
        }
    }

    pub fn ram_usage(&self) -> RamUsage {
        match self {
            DynamicValue::Custom(data) => data.ram_bytes(),
            _ => RamUsage::default(),
        }
    }
}

impl Display for DynamicValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DynamicValue::Unbound => write!(f, "-"),
            DynamicValue::Static(value) => write!(f, "{value}"),
            DynamicValue::Custom(data) => write!(f, "{data}"),
        }
    }
}

impl From<&StaticValue> for DynamicValue {
    fn from(value: &StaticValue) -> Self {
        DynamicValue::Static(value.clone())
    }
}

impl From<StaticValue> for DynamicValue {
    fn from(value: StaticValue) -> Self {
        DynamicValue::Static(value)
    }
}

macro_rules! dynamic_from_static {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl From<$ty> for DynamicValue {
                fn from(value: $ty) -> Self {
                    DynamicValue::Static(value.into())
                }
            }
        )+
    };
}

dynamic_from_static!(i64, i32, f32, f64, String, bool);

impl From<&str> for DynamicValue {
    fn from(value: &str) -> Self {
        DynamicValue::Static(value.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Tag(&'static str);

    impl Display for Tag {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "tag:{}", self.0)
        }
    }

    impl CustomValue for Tag {
        fn type_id(&self) -> TypeId {
            TypeId::from_u128(0xaa)
        }

        fn as_any(&self) -> &dyn Any {
            self
        }

        fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
            self
        }
    }

    #[test]
    fn scalar_conversions_and_runtime_states() {
        let value: DynamicValue = 3.5f64.into();
        assert_eq!(value.as_f64(), Some(3.5));
        assert_eq!(DynamicValue::from(true).as_bool(), Some(true));
        assert_eq!(DynamicValue::Unbound.as_f64(), None);
        assert_eq!(DynamicValue::Unbound.to_value_string(), "");
        assert!(matches!(DynamicValue::default(), DynamicValue::Unbound));
        assert_eq!(
            DynamicValue::from_custom(Tag("z")).to_value_string(),
            "tag:z"
        );
    }

    #[test]
    fn into_custom_requires_the_right_unique_value() {
        let unique = DynamicValue::from_custom(Tag("solo"));
        assert_eq!(unique.into_custom::<Tag>().unwrap().0, "solo");

        let first = DynamicValue::from_custom(Tag("shared"));
        let second = first.clone();
        let returned = first.into_custom::<Tag>().unwrap_err();
        assert_eq!(returned.as_custom::<Tag>().unwrap().0, "shared");
        drop(second);
        assert_eq!(returned.into_custom::<Tag>().unwrap().0, "shared");

        assert!(matches!(
            DynamicValue::Unbound.into_custom::<Tag>().unwrap_err(),
            DynamicValue::Unbound
        ));
    }
}
