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
//! `DynamicValue`s. A config type carries its own wire identity, the
//! `Introspect` `TYPE_ID` and `DISPLAY_NAME` its derive declares — `lumos`
//! derives them on its own types — and where a config's shape does not fit the
//! field model, [`crate::astro::config`] declares a flat projection instead.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use common::{FieldKind, FieldValue, Introspect};
use scenarium::FuncLambda;
use scenarium::Invocation;
use scenarium::{ConstValue, CustomValue, DataType, DynamicValue, EnumVariants, TypeId};
use scenarium::{Func, FuncId, FuncInput, FuncOutput};
use scenarium::{InvokeError, Library, TypeEntry};

/// A config type's wire type id, parsed once at compile time from its `TYPE_ID`.
pub(crate) const fn wire_type_id<T: Introspect>() -> TypeId {
    const { TypeId::literal(T::TYPE_ID) }
}

/// A built config flowing on a wire — wraps the typed value.
#[derive(Debug)]
pub(crate) struct ConfigValue<T>(pub(crate) T);

impl<T: Introspect + Clone + fmt::Debug + Send + Sync + 'static> CustomValue for ConfigValue<T> {
    fn type_id(&self) -> TypeId {
        wire_type_id::<T>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

impl<T: fmt::Debug> fmt::Display for ConfigValue<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

/// The custom [`DataType`] a `T` config travels on (distinct per `T`, so wiring
/// is type-checked).
pub(crate) const fn config_data_type<T: Introspect>() -> DataType {
    DataType::Custom(wire_type_id::<T>())
}

/// A required enum/preset dropdown input seeded to `E`'s first variant. Shared by
/// both libraries: the default keeps a fresh node valid, while clearing it
/// surfaces as a missing input. (The first-variant default is read from `E`
/// directly, so this needs no library handle.)
pub(crate) fn enum_input<E: EnumVariants>(name: &str, datatype: &DataType) -> FuncInput {
    let mut input = FuncInput::required(name, datatype.clone());
    input.default_value = E::variant_names().into_iter().next().map(ConstValue::Enum);
    input
}

/// Build a config-builder `Func` for `T` — one labeled input per introspected
/// field, a single `config` output of [`config_data_type::<T>`] — and add it to
/// `library`, registering `T`'s output type and any enum field types so the
/// editor can render them. Adds internally (rather than returning the `Func`) so
/// the caller needn't borrow `library` twice.
pub(crate) fn add_config_builder<T: Introspect + Clone + fmt::Debug + Send + Sync + 'static>(
    library: &mut Library,
    node_id: FuncId,
    node_name: &str,
    description: &str,
) {
    let fields = T::fields();
    library.register_type(wire_type_id::<T>(), TypeEntry::custom(T::DISPLAY_NAME));
    for field in &fields {
        register_field_enum(library, &field.kind);
    }
    // The lambda needs each field's kind to read its input value back.
    let kinds: Arc<[FieldKind]> = fields.iter().map(|field| field.kind).collect();
    let lambda = FuncLambda::new(
        move |Invocation {
                  inputs, outputs, ..
              }| {
            let kinds = Arc::clone(&kinds);
            Box::pin(async move {
                let values: Vec<FieldValue> = kinds
                    .iter()
                    .zip(inputs)
                    .map(|(kind, input)| field_value(kind, input))
                    .collect();
                let config = T::from_fields(&values).map_err(InvokeError::external)?;
                outputs[0] = DynamicValue::from_custom(ConfigValue(config));
                Ok(())
            })
        },
    );
    let mut func = Func::new(node_id, node_name, lambda)
        .category("Astro")
        .description(description)
        .pure();
    for field in &fields {
        let data_type = data_type(&field.kind);
        let input = if field.required {
            FuncInput::required(field.label, data_type)
        } else {
            FuncInput::optional(field.label, data_type)
        };
        func = func.input(input.default(const_value(&field.default)));
    }
    library.add(func.output(FuncOutput::new("Config", config_data_type::<T>())));
}

/// Map an introspected field kind to a scenarium port type. Enum fields map to
/// `DataType::Enum(id)`; their metadata is registered separately by
/// [`register_field_enum`].
fn data_type(kind: &FieldKind) -> DataType {
    match kind {
        FieldKind::Int(_) => DataType::Int,
        FieldKind::Float(_) => DataType::Float,
        FieldKind::Bool => DataType::Bool,
        FieldKind::Str => DataType::String,
        FieldKind::Enum { type_id, .. } => DataType::Enum(TypeId::literal(type_id)),
        // An `Option<T>` port is `T`'s type; optionality is the input's `required` flag.
        FieldKind::Option(inner) => data_type(inner),
    }
}

/// Register the enum type(s) a `kind` references on `library`. One enum can
/// appear across several config builders; `register_type` takes the identical
/// registration again and panics on conflicting metadata.
fn register_field_enum(library: &mut Library, kind: &FieldKind) {
    match kind {
        FieldKind::Enum {
            type_id,
            display_name,
            variants,
        } => {
            let id = TypeId::literal(type_id);
            let variants = variants.iter().map(|&variant| variant.to_owned()).collect();
            library.register_type(id, TypeEntry::enum_with_variants(*display_name, variants));
        }
        FieldKind::Option(inner) => register_field_enum(library, inner),
        _ => {}
    }
}

/// A neutral field default → an authored constant.
fn const_value(value: &FieldValue) -> ConstValue {
    match value {
        FieldValue::Int(n) => ConstValue::Int(
            i64::try_from(*n)
                .expect("introspected integer defaults must fit Scenarium's i64 value model"),
        ),
        FieldValue::Float(f) => ConstValue::Float(*f),
        FieldValue::Bool(b) => ConstValue::Bool(*b),
        FieldValue::Str(s) => ConstValue::String(s.clone()),
        FieldValue::Enum(v) => ConstValue::Enum(v.clone()),
        FieldValue::Null => ConstValue::Null,
    }
}

/// Read an input's runtime value back into a neutral field value.
fn field_value(kind: &FieldKind, value: &DynamicValue) -> FieldValue {
    match kind {
        FieldKind::Int(_) => FieldValue::Int(
            value
                .as_i64()
                .expect("integer config input type is validated at the compile boundary")
                .into(),
        ),
        FieldKind::Float(_) => FieldValue::Float(
            value
                .as_f64()
                .expect("float config input type is validated at the compile boundary"),
        ),
        FieldKind::Bool => FieldValue::Bool(
            value
                .as_bool()
                .expect("boolean config input type is validated at the compile boundary"),
        ),
        FieldKind::Str => FieldValue::Str(
            value
                .as_string()
                .expect("string config input type is validated at the compile boundary")
                .to_string(),
        ),
        FieldKind::Enum { .. } => FieldValue::Enum(
            value
                .as_enum()
                .expect("enum config input type is validated at the compile boundary")
                .to_string(),
        ),
        FieldKind::Option(_) if matches!(value, DynamicValue::Unbound) => FieldValue::Null,
        FieldKind::Option(_) if matches!(value.as_static(), Some(ConstValue::Null)) => {
            FieldValue::Null
        }
        FieldKind::Option(inner) => field_value(inner, value),
    }
}

#[cfg(test)]
mod tests {
    use common::{FieldKind, FieldValue, FloatKind, IntegerKind, IntegerValue};
    use scenarium::{ConstValue, DataType, DynamicValue, Library, TypeId};

    use crate::config_node::{data_type, field_value, register_field_enum};

    #[test]
    fn maps_field_kinds_to_port_types() {
        const TYPE_ID: &str = "fc62ecf4-d470-4731-a7a0-9a0bf944f782";
        assert!(matches!(
            data_type(&FieldKind::Int(IntegerKind::Usize)),
            DataType::Int
        ));
        assert!(matches!(data_type(&FieldKind::Bool), DataType::Bool));
        assert!(matches!(
            data_type(&FieldKind::Option(&FieldKind::Float(FloatKind::F32))),
            DataType::Float
        ));
        let kind = FieldKind::Enum {
            type_id: TYPE_ID,
            display_name: "Mode",
            variants: &["a", "b"],
        };
        let expected_id = TypeId::literal(TYPE_ID);
        assert_eq!(data_type(&kind), DataType::Enum(expected_id));
        let renamed = FieldKind::Enum {
            type_id: TYPE_ID,
            display_name: "Renamed Mode",
            variants: &["a", "b"],
        };
        assert_eq!(data_type(&renamed), DataType::Enum(expected_id));

        // Registration records the enum's name + variants under that id.
        let mut library = Library::default();
        register_field_enum(&mut library, &kind);
        register_field_enum(&mut library, &kind);
        let entry = library.type_entry(expected_id).unwrap();
        assert_eq!(entry.display_name(), "Mode");
        assert_eq!(
            entry.variants(),
            Some(["a".to_string(), "b".to_string()].as_slice())
        );
    }

    #[test]
    #[should_panic(expected = "conflicting registration of type")]
    fn rejects_disagreeing_metadata_for_one_enum_identity() {
        const TYPE_ID: &str = "5a779f56-4959-4321-86ef-0e98a6cbdd84";
        let first = FieldKind::Enum {
            type_id: TYPE_ID,
            display_name: "Mode",
            variants: &["a"],
        };
        let conflicting = FieldKind::Enum {
            type_id: TYPE_ID,
            display_name: "Mode",
            variants: &["b"],
        };
        let mut library = Library::default();
        register_field_enum(&mut library, &first);
        register_field_enum(&mut library, &conflicting);
    }

    #[test]
    fn reads_dynamic_values_by_kind() {
        assert_eq!(
            field_value(
                &FieldKind::Int(IntegerKind::Usize),
                &DynamicValue::Static(ConstValue::Int(5))
            ),
            FieldValue::Int(IntegerValue::Signed(5))
        );
        assert_eq!(
            field_value(
                &FieldKind::Option(&FieldKind::Int(IntegerKind::U32)),
                &DynamicValue::Static(ConstValue::Int(7))
            ),
            FieldValue::Int(IntegerValue::Signed(7))
        );
        assert_eq!(
            field_value(
                &FieldKind::Option(&FieldKind::Float(FloatKind::F64)),
                &DynamicValue::Unbound
            ),
            FieldValue::Null
        );
    }

    #[test]
    #[should_panic(expected = "float config input type is validated")]
    fn rejects_incompatible_required_values() {
        field_value(&FieldKind::Float(FloatKind::F64), &DynamicValue::Unbound);
    }
}
