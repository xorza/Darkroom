use crate::graph::identity::FuncId;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::CustomValueCodec;
use crate::data::codec::Codecs;
use crate::data::type_system::Strictness;
use crate::graph::func::{Func, FuncInput, OutputType};
use crate::{ConstValue, DataType, EnumVariants, TypeId};

#[derive(Clone, Debug)]
enum TypeEntryKind {
    Custom {
        display_name: String,
        codec: Option<Arc<dyn CustomValueCodec>>,
    },
    Enum {
        display_name: String,
        variants: Vec<String>,
    },
}

/// A registered nominal type. A custom type may carry a disk codec; an enum
/// carries only its display metadata and variants.
#[derive(Clone, Debug)]
pub struct TypeEntry {
    kind: TypeEntryKind,
}

impl TypeEntry {
    fn custom_entry(
        display_name: impl Into<String>,
        codec: Option<Arc<dyn CustomValueCodec>>,
    ) -> Self {
        Self {
            kind: TypeEntryKind::Custom {
                display_name: display_name.into(),
                codec,
            },
        }
    }

    /// A custom type with no disk codec (not cacheable).
    pub fn custom(display_name: impl Into<String>) -> Self {
        Self::custom_entry(display_name, None)
    }

    /// A custom type with a disk codec.
    pub fn custom_with_codec(
        display_name: impl Into<String>,
        codec: Arc<dyn CustomValueCodec>,
    ) -> Self {
        Self::custom_entry(display_name, Some(codec))
    }

    /// An enum type with the variant names taken from `E` (via strum).
    pub fn enum_of<E: EnumVariants>(display_name: impl Into<String>) -> Self {
        Self::enum_with_variants(display_name, E::variant_names())
    }

    /// An enum type with an explicit variant list (for runtime-discovered enums
    /// where the concrete type isn't available — see `lens`'s config builders).
    pub fn enum_with_variants(display_name: impl Into<String>, variants: Vec<String>) -> Self {
        Self {
            kind: TypeEntryKind::Enum {
                display_name: display_name.into(),
                variants,
            },
        }
    }

    pub fn display_name(&self) -> &str {
        match &self.kind {
            TypeEntryKind::Custom { display_name, .. }
            | TypeEntryKind::Enum { display_name, .. } => display_name,
        }
    }

    /// The variant names for an enum entry; `None` for a custom type.
    pub fn variants(&self) -> Option<&[String]> {
        match &self.kind {
            TypeEntryKind::Enum { variants, .. } => Some(variants),
            TypeEntryKind::Custom { .. } => None,
        }
    }

    fn codec(&self) -> Option<&Arc<dyn CustomValueCodec>> {
        match &self.kind {
            TypeEntryKind::Custom { codec, .. } => codec.as_ref(),
            TypeEntryKind::Enum { .. } => None,
        }
    }
}

/// The runtime registry every frontend resolves against: the [`Func`]s nodes
/// instantiate, and the nominal types (with their disk codecs). This is runtime registry state, not a persistence format;
/// authored graphs serialize function and type ids and resolve them against a
/// process-assembled library.
#[derive(Default, Debug)]
pub struct Library {
    funcs: HashMap<FuncId, Func>,

    /// Registered nominal types (`Custom`/`Enum`), keyed by [`TypeId`]. The home
    /// for type metadata and the disk codecs the output cache dispatches through.
    /// Lookup-only (never iterated in order), so a plain map rather than an
    /// ordered map.
    pub types: HashMap<TypeId, TypeEntry>,
}

impl Library {
    pub fn by_id(&self, id: FuncId) -> Option<&Func> {
        assert!(!id.is_nil());
        self.funcs.get(&id)
    }

    pub fn by_name(&self, name: &str) -> Option<&Func> {
        assert!(!name.is_empty());
        self.funcs().find(|func| func.name == name)
    }

    pub fn funcs(&self) -> impl ExactSizeIterator<Item = &Func> {
        self.funcs.values()
    }

    pub fn add(&mut self, func: Func) {
        func.validate().expect("invalid function declaration");
        assert!(
            !self.funcs.contains_key(&func.id),
            "duplicate function registration"
        );
        assert_enum_defaults(&func, |type_id| self.enum_variants(type_id));
        for type_id in declared_enum_types(&func) {
            assert!(
                !self.is_registered_custom(type_id),
                "function {:?} declares {type_id:?} as an enum, but it is registered as a \
                 custom type",
                func.name,
            );
        }
        self.funcs.insert(func.id, func);
    }

    /// Drop a func declaration, handing back what was registered under `id` —
    /// how a host assembles a library that omits an entry a shared builder
    /// added (lens drops its ML nodes when their backend is unavailable).
    pub fn remove(&mut self, id: FuncId) -> Option<Func> {
        self.funcs.remove(&id)
    }

    /// Register a nominal type. Panics on a duplicate id — two decls for one type
    /// is a wiring bug, not a runtime condition (as the old codec registry did).
    pub fn register_type(&mut self, type_id: impl Into<TypeId>, entry: TypeEntry) {
        let type_id = type_id.into();
        assert!(!type_id.is_nil());
        assert!(
            !self.types.contains_key(&type_id),
            "duplicate type registration"
        );
        // Funcs and their enum types register in either order, so the
        // membership gate runs from both directions: `add` checks against
        // types already present, and a fresh enum entry re-checks the funcs
        // already added.
        //
        // **Before the insert.** This gate panics, and installing first
        // left the rejected entry in `types` for every later lookup to
        // find — the registry kept exactly the declaration it had just
        // refused. Nothing is in the map yet, so the check resolves
        // `type_id` from `entry` directly; every *other* enum a func
        // declares was already gated when that type registered.
        match entry.variants() {
            Some(variants) => {
                for func in self.funcs.values() {
                    assert_enum_defaults(func, |declared| {
                        (declared == type_id).then_some(variants)
                    });
                }
            }
            // The other half of the same gate. `enum_variants` answers
            // `None` for "not registered yet" *and* for "registered as a
            // custom type", and deferred registration makes the first one
            // legitimate — so nothing rejected a func that declared this
            // id as an enum, and the mismatch only surfaced much later
            // and much quieter, as an enum const that failed
            // `const_satisfies` and lowered to unbound.
            None => {
                for func in self.funcs.values() {
                    assert!(
                        !declared_enum_types(func).any(|declared| declared == type_id),
                        "{type_id:?} is being registered as a custom type, but function {:?} \
                         declares it as an enum",
                        func.name,
                    );
                }
            }
        }
        self.types.insert(type_id, entry);
    }

    /// Whether `type_id` is registered as a **custom** type — the state
    /// [`Self::enum_variants`] cannot tell apart from "not registered at
    /// all", since it answers `None` to both.
    fn is_registered_custom(&self, type_id: TypeId) -> bool {
        self.types
            .get(&type_id)
            .is_some_and(|entry| entry.variants().is_none())
    }

    /// The variant names of a registered `Enum` type — for the editor's enum
    /// picker and the const type-check. `None` if `type_id` is unregistered or
    /// names a non-enum type.
    pub fn enum_variants(&self, type_id: TypeId) -> Option<&[String]> {
        assert!(!type_id.is_nil());
        self.types.get(&type_id)?.variants()
    }

    /// A short human-readable name for `ty`: the scalar keyword, `"path"`, or a
    /// registered `Custom`/`Enum` type's display name (its raw id if the type
    /// isn't registered in this process).
    pub fn type_name(&self, ty: &DataType) -> Cow<'_, str> {
        match ty {
            DataType::Any => Cow::Borrowed("any"),
            DataType::Float => Cow::Borrowed("float"),
            DataType::Int => Cow::Borrowed("int"),
            DataType::Bool => Cow::Borrowed("bool"),
            DataType::String => Cow::Borrowed("string"),
            DataType::FsPath(_) => Cow::Borrowed("path"),
            DataType::Custom(id) | DataType::Enum(id) => self
                .types
                .get(id)
                .map(|entry| Cow::Borrowed(entry.display_name()))
                .unwrap_or_else(|| Cow::Owned(id.to_string())),
        }
    }

    /// Snapshot of the registered disk codecs — everything the output cache's
    /// serialize/deserialize needs from the library.
    pub(crate) fn codecs(&self) -> Codecs {
        Codecs {
            by_type: self
                .types
                .iter()
                .filter_map(|(id, entry)| Some((*id, Arc::clone(entry.codec()?))))
                .collect(),
        }
    }

    pub fn merge<T: Into<Library>>(&mut self, other: T) {
        let other = other.into();
        for func in other.funcs.into_values() {
            self.add(func);
        }
        for (type_id, entry) in other.types {
            self.register_type(type_id, entry);
        }
    }

    /// Whether an authored `Const` literal `value` may sit on `input` — the
    /// `Const` half of the lowering-time type degrade (the `Bind` half is
    /// [`DataType::compatible_with`]); a literal that doesn't satisfy its port
    /// lowers as unbound.
    ///
    /// The registry's whole contribution is the enum arm: a literal is a
    /// variant *name*, and only this map says which names a type declares.
    /// Everything else is [`FuncInput::accepts_const`], the one table both
    /// gates read.
    pub(crate) fn const_satisfies(&self, input: &FuncInput, value: &ConstValue) -> bool {
        input.accepts_const(value, Strictness::Authored, |type_id, name| {
            self.enum_variants(type_id)
                .is_some_and(|variants| variants.iter().any(|variant| variant == name))
        })
    }
}

/// Every type a `func` port declares as [`DataType::Enum`], inputs and
/// outputs alike. A wildcard output declares no nominal type of its
/// own, so it contributes nothing here.
fn declared_enum_types(func: &Func) -> impl Iterator<Item = TypeId> + '_ {
    let enum_id = |ty: &DataType| match ty {
        DataType::Enum(type_id) => Some(*type_id),
        _ => None,
    };
    let inputs = func
        .inputs
        .iter()
        .filter_map(move |i| enum_id(&i.data_type));
    let outputs = func.outputs.iter().filter_map(move |o| match &o.ty {
        OutputType::Fixed(ty) => enum_id(ty),
        OutputType::Wildcard { .. } => None,
    });
    inputs.chain(outputs)
}

/// Panic when `func` declares an `Enum` default whose name isn't among
/// its type's registered variants. Variant-kind and picker-membership
/// checks already ran in `Func::validate`; this closes the half that
/// needs the registry. A default on a type `variants_of` doesn't
/// resolve passes — `register_type` re-checks when the entry arrives.
///
/// Variants arrive through `variants_of` rather than off `&self`
/// because [`Library::register_type`] has to run this *before* its entry
/// is installed, so the type under test isn't in the map yet.
fn assert_enum_defaults<'a>(func: &Func, variants_of: impl Fn(TypeId) -> Option<&'a [String]>) {
    for input in &func.inputs {
        if !input.value_variants.is_empty() {
            continue;
        }
        let (DataType::Enum(type_id), Some(ConstValue::Enum(name))) =
            (&input.data_type, &input.default_value)
        else {
            continue;
        };
        if let Some(variants) = variants_of(*type_id) {
            assert!(
                variants.iter().any(|variant| variant == name),
                "function {:?} input {:?} defaults to {name:?}, which is not a registered \
                 variant of its enum type",
                func.name,
                input.name,
            );
        }
    }
}

impl<It> From<It> for Library
where
    It: IntoIterator<Item = Func>,
{
    fn from(iter: It) -> Self {
        let mut library = Library::default();
        for func in iter {
            library.add(func);
        }
        library
    }
}

#[cfg(test)]
mod tests;
