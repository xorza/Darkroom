use std::any;
use std::any::Any;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::ops::Add;
use std::ops::AddAssign;
use std::str::FromStr;
use std::sync::Arc;

use crate::{ConstValue, TypeId};

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

impl Add for RamUsage {
    type Output = RamUsage;

    fn add(self, rhs: RamUsage) -> Self::Output {
        RamUsage {
            cpu: self.cpu + rhs.cpu,
            gpu: self.gpu + rhs.gpu,
        }
    }
}

impl AddAssign for RamUsage {
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
    Static(ConstValue),
    Custom(Arc<dyn CustomValue>),
}

impl Debug for DynamicValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
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

    pub fn as_static(&self) -> Option<&ConstValue> {
        match self {
            DynamicValue::Static(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        self.as_static().and_then(ConstValue::as_f64)
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_static().and_then(ConstValue::as_i64)
    }

    pub fn as_bool(&self) -> Option<bool> {
        self.as_static().and_then(ConstValue::as_bool)
    }

    pub fn as_string(&self) -> Option<&str> {
        self.as_static().and_then(ConstValue::as_string)
    }

    pub fn as_enum(&self) -> Option<&str> {
        self.as_static().and_then(ConstValue::as_enum)
    }

    pub fn as_fs_path(&self) -> Option<&str> {
        self.as_static().and_then(ConstValue::as_fs_path)
    }

    pub fn as_fs_paths(&self) -> Option<&[String]> {
        self.as_static().and_then(ConstValue::as_fs_paths)
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

    /// The value of a required input declared `Float` (or any scalar the
    /// declaration coerces).
    ///
    /// The `required_*` reads state what the compiler guarantees a lambda: a
    /// node runs only with every required input bound to a value of its
    /// declared type. A lambda reading a required input as declared cannot
    /// fail; one that panics here read a port as a type it does not declare.
    #[track_caller]
    pub fn required_f64(&self) -> f64 {
        self.as_f64().unwrap_or_else(|| self.misread("a number"))
    }

    /// The value of a required input declared `Int`. See [`required_f64`](Self::required_f64).
    #[track_caller]
    pub fn required_i64(&self) -> i64 {
        self.as_i64().unwrap_or_else(|| self.misread("an integer"))
    }

    /// The value of a required input declared `Bool`. See [`required_f64`](Self::required_f64).
    #[track_caller]
    pub fn required_bool(&self) -> bool {
        self.as_bool().unwrap_or_else(|| self.misread("a boolean"))
    }

    /// The value of a required input declared `String`. See [`required_f64`](Self::required_f64).
    #[track_caller]
    pub fn required_string(&self) -> &str {
        self.as_string().unwrap_or_else(|| self.misread("a string"))
    }

    /// The variant of a required input declared `Enum`. See [`required_f64`](Self::required_f64).
    #[track_caller]
    pub fn required_enum(&self) -> &str {
        self.as_enum()
            .unwrap_or_else(|| self.misread("an enum variant"))
    }

    /// The variant of a required input declared `Enum`, parsed as `T`. The
    /// compiler checked the variant against the enum type's names, so this
    /// holds for a `T` that reads those names. See
    /// [`required_f64`](Self::required_f64).
    #[track_caller]
    pub fn required_enum_as<T: FromStr>(&self) -> T {
        self.required_enum()
            .parse()
            .unwrap_or_else(|_unread| self.misread(any::type_name::<T>()))
    }

    /// The path of a required input declared `FsPath` for one path. See
    /// [`required_f64`](Self::required_f64).
    #[track_caller]
    pub fn required_fs_path(&self) -> &str {
        self.as_fs_path().unwrap_or_else(|| self.misread("a path"))
    }

    /// The paths of a required input declared `FsPath`. See
    /// [`required_f64`](Self::required_f64).
    #[track_caller]
    pub fn required_fs_paths(&self) -> &[String] {
        self.as_fs_paths().unwrap_or_else(|| self.misread("paths"))
    }

    /// The value of a required input declared `Custom` as `T`. See
    /// [`required_f64`](Self::required_f64).
    #[track_caller]
    pub fn required_custom<T: CustomValue>(&self) -> &T {
        self.as_custom()
            .unwrap_or_else(|| self.misread(any::type_name::<T>()))
    }

    #[cold]
    #[track_caller]
    fn misread(&self, expected: &str) -> ! {
        panic!(
            "a required input holds {self:?}, not {expected}: the compiler delivers every \
             required input bound and of its declared type, so the lambda read a port as \
             another type"
        )
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
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            DynamicValue::Unbound => write!(f, "-"),
            DynamicValue::Static(value) => write!(f, "{value}"),
            DynamicValue::Custom(data) => write!(f, "{data}"),
        }
    }
}

impl From<&ConstValue> for DynamicValue {
    fn from(value: &ConstValue) -> Self {
        DynamicValue::Static(value.clone())
    }
}

impl From<ConstValue> for DynamicValue {
    fn from(value: ConstValue) -> Self {
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
    use std::panic;
    use std::panic::AssertUnwindSafe;

    use super::*;

    #[derive(Debug)]
    struct Tag(&'static str);

    /// An enum whose parse reads one name, `auto`.
    #[derive(Debug, PartialEq)]
    enum Pick {
        Auto,
    }

    impl FromStr for Pick {
        type Err = ();

        fn from_str(name: &str) -> Result<Self, ()> {
            (name == "auto").then_some(Pick::Auto).ok_or(())
        }
    }

    impl Display for Tag {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
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

    /// Each required read returns a value of its declared kind, a scalar
    /// coerced the way the declaration's coercion class allows, and panics,
    /// naming the misread, on anything else.
    #[test]
    fn required_reads_return_the_declared_kind_and_name_a_misread() {
        assert_eq!(DynamicValue::from(3i64).required_f64(), 3.0);
        assert_eq!(DynamicValue::from(2.75f64).required_f64(), 2.75);
        assert_eq!(DynamicValue::from(7i64).required_i64(), 7);
        assert!(DynamicValue::from(true).required_bool());
        let string = DynamicValue::Static(ConstValue::String("text".into()));
        assert_eq!(string.required_string(), "text");
        let variant = DynamicValue::Static(ConstValue::Enum("auto".into()));
        assert_eq!(variant.required_enum(), "auto");
        assert_eq!(variant.required_enum_as::<Pick>(), Pick::Auto);
        let other = DynamicValue::Static(ConstValue::Enum("manual".into()));
        assert!(
            panic::catch_unwind(AssertUnwindSafe(|| other.required_enum_as::<Pick>()))
                .unwrap_err()
                .downcast_ref::<String>()
                .unwrap()
                .contains(any::type_name::<Pick>())
        );
        let path = DynamicValue::Static(ConstValue::FsPath("a.fits".into()));
        assert_eq!(path.required_fs_path(), "a.fits");
        assert_eq!(path.required_fs_paths(), ["a.fits"]);
        let tag = DynamicValue::from_custom(Tag("t"));
        assert_eq!(tag.required_custom::<Tag>().0, "t");

        let message = |read: fn(&DynamicValue) -> String| {
            let panic = panic::catch_unwind(|| read(&DynamicValue::Unbound)).unwrap_err();
            panic.downcast_ref::<String>().cloned().unwrap()
        };
        assert!(
            message(|v| v.required_f64().to_string())
                .starts_with("a required input holds Unbound, not a number")
        );
        assert!(
            message(|v| v.required_string().to_owned())
                .starts_with("a required input holds Unbound, not a string")
        );
        assert!(
            message(|v| v.required_custom::<Tag>().0.to_owned()).contains(any::type_name::<Tag>())
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
