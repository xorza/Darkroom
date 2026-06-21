//! Generic config-builder nodes.
//!
//! [`config_builder_func`] turns any `serde` + [`schemars::JsonSchema`] config
//! type into a `Func` whose inputs are the type's fields — reflected from its
//! JSON schema — and whose output is the built config as a wireable
//! [`ConfigValue`]. So a consuming node can take either a quick preset *or* a
//! detailed config wired from a builder node, without hand-writing one input per
//! field.
//!
//! The reflected types are lens-local *mirror* structs (see
//! [`crate::astro_configs`]) — lumos stays free of `schemars`/serde-for-UI; each
//! mirror converts to its lumos config.
//!
//! - **Labels:** each input shows a human label — the field's `#[schemars(title
//!   = "…")]` if set, else the field name prettified (`tile_size` → "Tile
//!   Size"). The serde field name is kept for the build round-trip.
//! - **Required:** a field is required unless it's an `Option<_>` (schemars omits
//!   `Option`s from the schema's `required` set).
//! - **Scope:** flat configs — primitives + unit enums (→ dropdown). Nested
//!   structs, `Vec`, and data-carrying enums are skipped (the type's `Default`
//!   fills them).

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Number, Value};

use scenarium::data::{CustomValue, DataType, DynamicValue, EnumDef, StaticValue, TypeDef, TypeId};
use scenarium::func_lambda::{FuncLambda, InvokeInput};
use scenarium::function::{Func, FuncInput};

/// A config type that can back a generic config-builder node.
pub trait NodeConfig:
    Default + Clone + Serialize + DeserializeOwned + JsonSchema + Send + Sync + 'static
{
    /// Stable type id for the built config's wire (a `uuidgen` literal).
    const TYPE_ID: &'static str;
    /// Display name for the wire type (e.g. `"BackgroundConfig"`).
    const NAME: &'static str;
}

/// A built config flowing on a wire — wraps the typed value.
#[derive(Debug)]
pub struct ConfigValue<T>(pub T);

impl<T: NodeConfig> CustomValue for ConfigValue<T> {
    fn type_def(&self) -> Arc<TypeDef> {
        Arc::new(TypeDef {
            type_id: T::TYPE_ID.into(),
            display_name: T::NAME.to_string(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl<T: NodeConfig> fmt::Display for ConfigValue<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The config's fields as compact JSON — enough for the inspector.
        match serde_json::to_string(&self.0) {
            Ok(json) => f.write_str(&json),
            Err(_) => f.write_str(T::NAME),
        }
    }
}

/// The custom [`DataType`] a `T` config travels on (distinct per `T`, so wiring
/// is type-checked).
pub fn config_data_type<T: NodeConfig>() -> DataType {
    DataType::from_custom(T::TYPE_ID, T::NAME)
}

/// Build a config-builder `Func` for `T`: one labeled input per reflected field,
/// a single `config` output of [`config_data_type::<T>`].
pub fn config_builder_func<T: NodeConfig>(
    node_id: &str,
    node_name: &str,
    description: &str,
) -> Func {
    let fields = reflect_fields::<T>();
    let mut func = Func::new(node_id, node_name)
        .category("astro")
        .description(description)
        .pure();
    for field in &fields {
        let input = if field.required {
            FuncInput::required(&field.label, field.data_type.clone())
        } else {
            FuncInput::optional(&field.label, field.data_type.clone())
        };
        func = func.input(input.default(field.default.clone()));
    }
    func.output("config", config_data_type::<T>())
        .lambda(FuncLambda::new(move |_, _, _, inputs, _, outputs| {
            let fields = fields.clone();
            Box::pin(async move {
                assert_eq!(inputs.len(), fields.len());
                let config: T = build_from_inputs(&fields, inputs).map_err(anyhow::Error::from)?;
                outputs[0] = DynamicValue::from_custom(ConfigValue(config));
                Ok(())
            })
        }))
}

/// One reflected config field.
#[derive(Clone, Debug)]
struct Field {
    /// The serde field name — the JSON key for the build round-trip.
    serde_name: String,
    /// Human label shown on the input port.
    label: String,
    /// Required unless the field is an `Option<_>`.
    required: bool,
    data_type: DataType,
    default: StaticValue,
}

/// Reflect `T`'s top-level fields from its JSON schema + `Default` instance.
/// Fields whose type isn't a supported scalar/unit-enum are skipped (the
/// `Default` value fills them on build).
fn reflect_fields<T: NodeConfig>() -> Vec<Field> {
    let schema = serde_json::to_value(schemars::schema_for!(T)).expect("schema serializes");
    let defaults = serde_json::to_value(T::default()).expect("config serializes");

    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let defs = schema.get("$defs").and_then(Value::as_object);

    // schemars lists exactly the non-`Option` fields in `required`, in struct
    // order. Take those first, then any remaining (the `Option`s).
    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut names: Vec<String> = required
        .iter()
        .filter(|n| props.contains_key(*n))
        .cloned()
        .collect();
    for key in props.keys() {
        if !names.contains(key) {
            names.push(key.clone());
        }
    }

    names
        .into_iter()
        .filter_map(|name| {
            let prop = &props[&name];
            let (resolved, def_name) = resolve_ref(prop, defs);
            let data_type = field_data_type(&resolved, def_name.as_deref().unwrap_or(&name))?;
            let default = field_default(&data_type, defaults.get(&name));
            let label = prop
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| prettify(&name));
            let required = required.contains(&name);
            Some(Field {
                serde_name: name,
                label,
                required,
                data_type,
                default,
            })
        })
        .collect()
}

/// Resolve a `$ref` against `$defs`, returning the target schema + its def name;
/// pass non-refs through unchanged.
fn resolve_ref(schema: &Value, defs: Option<&Map<String, Value>>) -> (Value, Option<String>) {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference
            .rsplit('/')
            .next()
            .unwrap_or(reference)
            .to_string();
        if let Some(def) = defs.and_then(|d| d.get(&name)) {
            return (def.clone(), Some(name));
        }
    }
    (schema.clone(), None)
}

/// Map a resolved property schema to a [`DataType`]. `None` for unsupported
/// shapes (object / array / data-carrying enum).
fn field_data_type(schema: &Value, type_name: &str) -> Option<DataType> {
    if let Some(variants) = unit_enum_variants(schema) {
        return Some(DataType::Enum(Arc::new(EnumDef {
            type_id: stable_type_id(type_name),
            display_name: type_name.to_string(),
            variants,
        })));
    }
    // `Option<T>` is `{"type": ["T", "null"]}` — take the non-null member.
    match schema_type(schema)?.as_str() {
        "integer" => Some(DataType::Int),
        "number" => Some(DataType::Float),
        "boolean" => Some(DataType::Bool),
        "string" => Some(DataType::String),
        _ => None,
    }
}

/// The scalar JSON-schema `type` of a property, stripping a `"null"` companion
/// (so `Option<T>` reports `T`'s type). `None` if absent or not scalar.
fn schema_type(schema: &Value) -> Option<String> {
    match schema.get("type") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .find(|s| *s != "null")
            .map(str::to_string),
        _ => None,
    }
}

/// Variant names of a unit-only enum schema (schemars emits `oneOf` of
/// `{const: "..."}`; the bare `enum` form is handled too). `None` otherwise.
fn unit_enum_variants(schema: &Value) -> Option<Vec<String>> {
    if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array) {
        let variants: Vec<String> = one_of
            .iter()
            .filter_map(|v| v.get("const").and_then(Value::as_str).map(str::to_string))
            .collect();
        // Every branch must be a bare `const` — otherwise it's a data-carrying enum.
        if !variants.is_empty() && variants.len() == one_of.len() {
            return Some(variants);
        }
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let variants: Vec<String> = values
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        if !variants.is_empty() && variants.len() == values.len() {
            return Some(variants);
        }
    }
    None
}

/// The default constant for a reflected field, from the type's `Default` JSON.
/// A null/absent default (an `Option` set to `None`) becomes [`StaticValue::Null`].
fn field_default(data_type: &DataType, default: Option<&Value>) -> StaticValue {
    if let Some(Value::Null) = default {
        return StaticValue::Null;
    }
    match data_type {
        DataType::Int => StaticValue::Int(default.and_then(Value::as_i64).unwrap_or(0)),
        DataType::Float => StaticValue::Float(default.and_then(Value::as_f64).unwrap_or(0.0)),
        DataType::Bool => StaticValue::Bool(default.and_then(Value::as_bool).unwrap_or(false)),
        DataType::String => StaticValue::String(
            default
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        DataType::Enum(def) => StaticValue::Enum(
            default
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| def.variants[0].clone()),
        ),
        _ => StaticValue::Null,
    }
}

/// Reassemble `T` from the node's field inputs: each input value → its JSON form
/// (coerced to the field's type, default on miss) keyed by the serde field name
/// → `from_value`.
fn build_from_inputs<T: DeserializeOwned>(
    fields: &[Field],
    inputs: &[InvokeInput],
) -> serde_json::Result<T> {
    let mut obj = Map::new();
    for (field, input) in fields.iter().zip(inputs) {
        obj.insert(field.serde_name.clone(), input_to_json(field, &input.value));
    }
    serde_json::from_value(Value::Object(obj))
}

fn input_to_json(field: &Field, value: &DynamicValue) -> Value {
    match &field.data_type {
        DataType::Int => value.as_i64().map(Value::from),
        DataType::Float => value.as_f64().and_then(Number::from_f64).map(Value::Number),
        DataType::Bool => value.as_bool().map(Value::from),
        DataType::String => value.as_string().map(|s| Value::from(s.to_string())),
        DataType::Enum(_) => value.as_enum().map(|s| Value::from(s.to_string())),
        _ => None,
    }
    // An unset optional falls back to its default — `Null` for an `Option` →
    // deserializes back to `None`.
    .unwrap_or_else(|| static_to_json(&field.default))
}

fn static_to_json(value: &StaticValue) -> Value {
    match value {
        StaticValue::Int(n) => Value::from(*n),
        StaticValue::Float(f) => Number::from_f64(*f).map_or(Value::Null, Value::Number),
        StaticValue::Bool(b) => Value::from(*b),
        StaticValue::String(s) | StaticValue::FsPath(s) => Value::from(s.clone()),
        StaticValue::Enum(v) => Value::from(v.clone()),
        StaticValue::Null => Value::Null,
    }
}

/// `snake_case` field name → "Title Case" label.
fn prettify(name: &str) -> String {
    name.split('_')
        .filter(|w| !w.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            let first = chars.next().unwrap_or_default().to_uppercase();
            format!("{first}{}", chars.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A stable, content-derived [`TypeId`] for a reflected enum type (FNV-1a of the
/// type name). Not a `uuidgen` literal — these enum datatypes are generated, not
/// persisted as a shared identity (bindings serialize the variant name only).
fn stable_type_id(name: &str) -> TypeId {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    TypeId::from_u128(hash as u128)
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
    #[serde(rename_all = "snake_case")]
    enum Mode {
        #[default]
        Fast,
        Slow,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
    struct TestConfig {
        tile_size: u32,
        #[schemars(title = "Custom Label")]
        threshold: f32,
        mode: Mode,
        enabled: bool,
        limit: Option<u32>,
    }

    impl Default for TestConfig {
        fn default() -> Self {
            Self {
                tile_size: 128,
                threshold: 2.5,
                mode: Mode::Fast,
                enabled: true,
                limit: None,
            }
        }
    }

    impl NodeConfig for TestConfig {
        const TYPE_ID: &'static str = "00000000-0000-0000-0000-000000000001";
        const NAME: &'static str = "TestConfig";
    }

    #[test]
    fn reflects_labels_types_and_required() {
        let fields = reflect_fields::<TestConfig>();
        // Required fields (struct order) first, the `Option` last; labels are
        // prettified field names, overridden by `#[schemars(title)]`.
        let labels: Vec<&str> = fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Tile Size", "Custom Label", "Mode", "Enabled", "Limit"]
        );
        assert_eq!(fields[0].serde_name, "tile_size");

        assert!(matches!(fields[0].data_type, DataType::Int));
        assert!(matches!(fields[1].data_type, DataType::Float));
        let DataType::Enum(def) = &fields[2].data_type else {
            panic!("mode should reflect as an enum");
        };
        assert_eq!(def.variants, ["fast", "slow"]);
        assert!(matches!(fields[3].data_type, DataType::Bool));
        assert!(matches!(fields[4].data_type, DataType::Int)); // Option<u32>

        // Only the `Option` field is optional.
        assert!(fields[..4].iter().all(|f| f.required));
        assert!(!fields[4].required, "Option field must be optional");

        assert_eq!(fields[0].default, StaticValue::Int(128));
        assert_eq!(fields[2].default, StaticValue::Enum("fast".to_string()));
        assert_eq!(fields[3].default, StaticValue::Bool(true));
        assert_eq!(fields[4].default, StaticValue::Null); // None
    }

    #[test]
    fn rebuilds_config_from_default_inputs() {
        let fields = reflect_fields::<TestConfig>();
        let inputs: Vec<InvokeInput> = fields
            .iter()
            .map(|f| InvokeInput {
                changed: true,
                value: DynamicValue::Static(f.default.clone()),
            })
            .collect();
        let built: TestConfig = build_from_inputs(&fields, &inputs).unwrap();
        assert_eq!(built, TestConfig::default());
    }

    #[test]
    fn edited_inputs_override_defaults() {
        let fields = reflect_fields::<TestConfig>();
        let mut inputs: Vec<InvokeInput> = fields
            .iter()
            .map(|f| InvokeInput {
                changed: true,
                value: DynamicValue::Static(f.default.clone()),
            })
            .collect();
        inputs[2].value = DynamicValue::Static(StaticValue::Enum("slow".to_string()));
        inputs[4].value = DynamicValue::Static(StaticValue::Int(7)); // Some(7)
        let built: TestConfig = build_from_inputs(&fields, &inputs).unwrap();
        assert_eq!(built.mode, Mode::Slow);
        assert_eq!(built.limit, Some(7));
    }
}
