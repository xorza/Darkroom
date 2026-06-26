//! Generic config-builder nodes — the scenarium bridge over
//! [`common`]'s struct introspection.
//!
//! [`config_builder_func`] turns any [`common::Introspect`] config type into a
//! `Func` whose inputs are the type's fields (mapped from [`common::FieldDesc`]
//! to scenarium [`DataType`]s) and whose output is the built config as a
//! wireable [`ConfigValue`]. So a consuming node can take either a quick preset
//! *or* a detailed config wired from a builder node.
//!
//! The introspection itself (field reflection, labels, typed rebuild) lives in
//! `common` and is GUI-agnostic; this module only maps it to node ports +
//! `DynamicValue`s. A config type is a [`NodeConfig`]: `Introspect` plus a
//! stable wire `TYPE_ID`/`NAME`. See [`crate::astro::configs`] for the mirror
//! types.

use std::any::Any;
use std::fmt;
use std::sync::LazyLock;

use common::{FieldKind, FieldValue, Introspect};
use scenarium::data::{CustomValue, DataType, DynamicValue, EnumVariants, StaticValue, TypeId};
use scenarium::func_lambda::FuncLambda;
use scenarium::function::{Func, FuncInput};
use scenarium::library::{Library, TypeEntry};

/// A config type that can back a config-builder node: introspectable, plus a
/// stable identity for the value it travels on.
pub(crate) trait NodeConfig:
    Introspect + Clone + fmt::Debug + Send + Sync + 'static
{
    /// Stable type id for the built config's wire (a `uuidgen` literal).
    const TYPE_ID: &'static str;
    /// Display name for the wire type (e.g. `"BackgroundConfig"`).
    const NAME: &'static str;
}

/// A built config flowing on a wire — wraps the typed value.
#[derive(Debug)]
pub(crate) struct ConfigValue<T>(pub(crate) T);

impl<T: NodeConfig> CustomValue for ConfigValue<T> {
    fn type_id(&self) -> TypeId {
        T::TYPE_ID.into()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl<T: NodeConfig> fmt::Display for ConfigValue<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

/// The custom [`DataType`] a `T` config travels on (distinct per `T`, so wiring
/// is type-checked).
pub(crate) fn config_data_type<T: NodeConfig>() -> DataType {
    DataType::Custom(T::TYPE_ID.into())
}

/// A required enum/preset dropdown input seeded to `E`'s first variant. Shared by
/// both libraries: the default keeps a fresh node valid, while clearing it
/// surfaces as a missing input. (The first-variant default is read from `E`
/// directly, so this needs no library handle.)
pub(crate) fn enum_input<E: EnumVariants>(name: &str, datatype: &DataType) -> FuncInput {
    let mut input = FuncInput::required(name, datatype.clone());
    input.default_value = E::variant_names().into_iter().next().map(StaticValue::Enum);
    input
}

/// Build a config-builder `Func` for `T` — one labeled input per introspected
/// field, a single `config` output of [`config_data_type::<T>`] — and add it to
/// `library`, registering `T`'s output type and any enum field types so the
/// editor can render them. Adds internally (rather than returning the `Func`) so
/// the caller needn't borrow `library` twice.
pub(crate) fn add_config_builder<T: NodeConfig>(
    library: &mut Library,
    node_id: &str,
    node_name: &str,
    description: &str,
) {
    let fields = T::fields();
    library.register_type(T::TYPE_ID, TypeEntry::custom(T::NAME));
    for field in &fields {
        register_field_enum(library, &field.kind);
    }
    let mut func = Func::new(node_id, node_name)
        .category("astro")
        .description(description)
        .pure();
    for field in &fields {
        let data_type = data_type(&field.kind);
        let input = if field.required {
            FuncInput::required(&field.label, data_type)
        } else {
            FuncInput::optional(&field.label, data_type)
        };
        func = func.input(input.default(static_value(&field.default)));
    }
    // The lambda needs each field's kind to read its input value back.
    let kinds: Vec<FieldKind> = fields.iter().map(|f| f.kind.clone()).collect();
    let func = func
        .output("config", config_data_type::<T>())
        .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            let kinds = kinds.clone();
            Box::pin(async move {
                let values: Vec<FieldValue> = kinds
                    .iter()
                    .zip(inputs)
                    .map(|(kind, input)| field_value(kind, &input.value))
                    .collect();
                outputs[0] = DynamicValue::from_custom(ConfigValue(T::from_fields(&values)));
                Ok(())
            })
        }));
    library.add(func);
}

/// Map an introspected field kind to a scenarium port type. Enum fields map to
/// `DataType::Enum(id)`; their metadata is registered separately by
/// [`register_field_enum`].
fn data_type(kind: &FieldKind) -> DataType {
    match kind {
        FieldKind::Int => DataType::Int,
        FieldKind::Float => DataType::Float,
        FieldKind::Bool => DataType::Bool,
        FieldKind::Str => DataType::String,
        FieldKind::Enum { type_name, .. } => DataType::Enum(stable_type_id(type_name)),
        // An `Option<T>` port is `T`'s type; optionality is the input's `required` flag.
        FieldKind::Option(inner) => data_type(inner),
    }
}

/// Register the enum type(s) a `kind` references on `library`. Idempotent — a
/// mirror enum can appear across several config builders, so a second
/// registration of the same id is a no-op rather than a panic.
fn register_field_enum(library: &mut Library, kind: &FieldKind) {
    match kind {
        FieldKind::Enum {
            type_name,
            variants,
        } => {
            let id = stable_type_id(type_name);
            if library.type_decl(&id).is_none() {
                library.register_type(
                    id,
                    TypeEntry::enum_with_variants(type_name, variants.clone()),
                );
            }
        }
        FieldKind::Option(inner) => register_field_enum(library, inner),
        _ => {}
    }
}

/// A neutral field default → an authored constant.
fn static_value(value: &FieldValue) -> StaticValue {
    match value {
        FieldValue::Int(n) => StaticValue::Int(*n),
        FieldValue::Float(f) => StaticValue::Float(*f),
        FieldValue::Bool(b) => StaticValue::Bool(*b),
        FieldValue::Str(s) => StaticValue::String(s.clone()),
        FieldValue::Enum(v) => StaticValue::Enum(v.clone()),
        FieldValue::Null => StaticValue::Null,
    }
}

/// Read an input's runtime value back into a neutral field value (per the
/// field's kind). An unset/incompatible value reads as `Null` → the rebuild
/// falls back to that field's default.
fn field_value(kind: &FieldKind, value: &DynamicValue) -> FieldValue {
    match kind {
        FieldKind::Int => value
            .as_i64()
            .map(FieldValue::Int)
            .unwrap_or(FieldValue::Null),
        FieldKind::Float => value
            .as_f64()
            .map(FieldValue::Float)
            .unwrap_or(FieldValue::Null),
        FieldKind::Bool => value
            .as_bool()
            .map(FieldValue::Bool)
            .unwrap_or(FieldValue::Null),
        FieldKind::Str => value
            .as_string()
            .map(|s| FieldValue::Str(s.to_string()))
            .unwrap_or(FieldValue::Null),
        FieldKind::Enum { .. } => value
            .as_enum()
            .map(|s| FieldValue::Enum(s.to_string()))
            .unwrap_or(FieldValue::Null),
        FieldKind::Option(inner) => field_value(inner, value),
    }
}

/// A stable [`TypeId`] for a reflected enum datatype, as a UUIDv5 of the enum's
/// name under [`ENUM_TYPE_NAMESPACE`] — deterministic across runs, full-width and
/// collision-resistant. These types are generated from introspection, not
/// authored, so they carry no `uuidgen` literal of their own (bindings serialize
/// the variant name only); the namespace literal supplies the entropy.
fn stable_type_id(name: &str) -> TypeId {
    TypeId::from_name(*ENUM_TYPE_NAMESPACE, name)
}

/// Fixed namespace for [`stable_type_id`]'s UUIDv5 enum-type ids (a `uuidgen`
/// literal). Distinct from any authored type id, so a reflected enum can't
/// collide with a hand-minted one.
static ENUM_TYPE_NAMESPACE: LazyLock<TypeId> =
    LazyLock::new(|| "48cbf7b0-f118-4ad8-a3c7-c2f71e8e555e".into());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_field_kinds_to_port_types() {
        assert!(matches!(data_type(&FieldKind::Int), DataType::Int));
        assert!(matches!(data_type(&FieldKind::Bool), DataType::Bool));
        // An Option port takes the inner type.
        assert!(matches!(
            data_type(&FieldKind::Option(Box::new(FieldKind::Float))),
            DataType::Float
        ));
        let kind = FieldKind::Enum {
            type_name: "Mode".to_string(),
            variants: vec!["a".to_string(), "b".to_string()],
        };
        assert_eq!(data_type(&kind), DataType::Enum(stable_type_id("Mode")));

        // Registration records the enum's name + variants under that id.
        let mut library = Library::default();
        register_field_enum(&mut library, &kind);
        let decl = library.type_decl(&stable_type_id("Mode")).unwrap();
        assert_eq!(decl.display_name(), "Mode");
        assert_eq!(
            decl.variants(),
            Some(["a".to_string(), "b".to_string()].as_slice())
        );
    }

    #[test]
    fn reads_dynamic_values_by_kind() {
        assert_eq!(
            field_value(&FieldKind::Int, &DynamicValue::Static(StaticValue::Int(5))),
            FieldValue::Int(5)
        );
        assert_eq!(
            field_value(
                &FieldKind::Option(Box::new(FieldKind::Int)),
                &DynamicValue::Static(StaticValue::Int(7))
            ),
            FieldValue::Int(7)
        );
        // Unset → Null (rebuild then keeps the field default).
        assert_eq!(
            field_value(&FieldKind::Float, &DynamicValue::Unbound),
            FieldValue::Null
        );
    }
}
