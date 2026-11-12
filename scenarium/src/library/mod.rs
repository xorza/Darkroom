pub(crate) mod type_entry;

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::Values;
use std::sync::Arc;

use crate::data::codec::Codecs;
use crate::data::const_value::ConstValue;
use crate::data::data_type::{DataType, Strictness, TypeId};
use crate::graph::error::Unresolved;
use crate::graph::func::Func;
use crate::graph::func::func_input::FuncInput;
use crate::graph::identity::FuncId;
use crate::graph::node::{Node, NodeKind};
use crate::library::type_entry::TypeEntry;

/// The runtime registry every frontend resolves against: the [`Func`]s nodes instantiate, and the
/// nominal types (with their disk codecs). Not a persistence format: authored graphs serialize
/// function and type ids and resolve them against a process-assembled library.
#[derive(Default, Debug)]
pub struct Library {
    funcs: HashMap<FuncId, Func>,

    /// Registered nominal types (`Custom`/`Enum`), written only by
    /// [`register_type`](Self::register_type).
    types: HashMap<TypeId, TypeEntry>,
    /// The codecs among `types`, shared with each program compiled from this library;
    /// registration copies the map only while a program still holds the previous one.
    codecs: Arc<Codecs>,
}

impl Library {
    /// The func with `id`, or `None`, as for the nil id, which
    /// [`FuncBuilder::build`](crate::FuncBuilder::build) refuses.
    pub fn by_id(&self, id: FuncId) -> Option<&Func> {
        self.funcs.get(&id)
    }

    /// The declaration `node` instantiates, or why it has none: no func under its id, or other
    /// ports than the node was authored against.
    pub fn func_of(&self, node: &Node) -> Result<&Func, Unresolved> {
        match &node.kind {
            NodeKind::Func(func_ref) => match self.by_id(func_ref.id()) {
                Some(func) if func.func_ref() == *func_ref => Ok(func),
                Some(_) => Err(Unresolved::ChangedPorts),
                None => Err(Unresolved::MissingFunc),
            },
            NodeKind::Special(special) => Ok(special.func()),
        }
    }

    pub fn by_name(&self, name: &str) -> Option<&Func> {
        self.iter().find(|func| func.name() == name)
    }

    /// The registered funcs, in no particular order.
    pub fn iter(&self) -> Values<'_, FuncId, Func> {
        self.funcs.values()
    }

    pub fn add(&mut self, func: Func) {
        assert!(
            !self.funcs.contains_key(&func.id()),
            "duplicate function registration"
        );
        assert_enum_defaults(&func, |type_id| self.enum_variants(type_id));
        for type_id in declared_enum_types(&func) {
            assert!(
                !self.is_registered_custom(type_id),
                "function {:?} declares {type_id:?} as an enum, but it is registered as a \
                 custom type",
                func.name(),
            );
        }
        self.funcs.insert(func.id(), func);
    }

    /// Register a nominal type. Registering an identical entry again does nothing, so libraries
    /// that share a type merge; a conflicting entry under a registered id panics.
    pub fn register_type(&mut self, type_id: impl Into<TypeId>, entry: TypeEntry) {
        let type_id = type_id.into();
        assert!(!type_id.is_nil());
        if let Some(existing) = self.types.get(&type_id) {
            assert!(
                existing.same_as(&entry),
                "conflicting registration of type {type_id:?}"
            );
            return;
        }
        // Funcs and their enum types register in either order, so the membership gate runs from
        // both directions: `add` checks against types already present, and a fresh enum entry
        // re-checks the funcs already added. Before the insert, so a refused entry never reaches
        // `types`; the check resolves `type_id` from `entry` directly.
        match entry.variants() {
            Some(variants) =>
            {
                #[expect(
                    clippy::iter_over_hash_type,
                    reason = "the order changes only which func an assert names first"
                )]
                for func in self.funcs.values() {
                    assert_enum_defaults(func, |declared| {
                        (declared == type_id).then_some(variants)
                    });
                }
            }
            // `enum_variants` answers `None` for "not registered yet" *and* "registered as a
            // custom type", and deferred registration makes the first legitimate, so `add`
            // cannot refuse a func that declares this id as an enum; this registration has to.
            None =>
            {
                #[expect(
                    clippy::iter_over_hash_type,
                    reason = "the order changes only which func an assert names first"
                )]
                for func in self.funcs.values() {
                    assert!(
                        !declared_enum_types(func).any(|declared| declared == type_id),
                        "{type_id:?} is being registered as a custom type, but function {:?} \
                         declares it as an enum",
                        func.name(),
                    );
                }
            }
        }
        if let Some(codec) = entry.codec() {
            Arc::make_mut(&mut self.codecs).insert(type_id, Arc::clone(codec));
        }
        self.types.insert(type_id, entry);
    }

    /// The registered entry of `type_id`, if any.
    pub fn type_entry(&self, type_id: TypeId) -> Option<&TypeEntry> {
        self.types.get(&type_id)
    }

    /// Every registered type.
    pub fn types(&self) -> impl ExactSizeIterator<Item = (&TypeId, &TypeEntry)> {
        self.types.iter()
    }

    /// Whether `type_id` is registered as a **custom** type, which [`Self::enum_variants`]
    /// cannot tell from "not registered at all".
    fn is_registered_custom(&self, type_id: TypeId) -> bool {
        self.types
            .get(&type_id)
            .is_some_and(|entry| entry.variants().is_none())
    }

    /// The variant names of a registered `Enum` type; `None` if `type_id` is unregistered or
    /// names a non-enum type.
    pub fn enum_variants(&self, type_id: TypeId) -> Option<&[String]> {
        self.types.get(&type_id)?.variants()
    }

    /// A short human-readable name for `ty`: the scalar keyword, `"path"`, or a registered
    /// `Custom`/`Enum` type's display name (its raw id if unregistered).
    pub fn type_name(&self, ty: &DataType) -> Cow<'_, str> {
        match ty {
            DataType::Any => Cow::Borrowed("any"),
            DataType::Float => Cow::Borrowed("float"),
            DataType::Int => Cow::Borrowed("int"),
            DataType::Bool => Cow::Borrowed("bool"),
            DataType::String => Cow::Borrowed("string"),
            DataType::FsPath(_) => Cow::Borrowed("path"),
            DataType::Custom(id) | DataType::Enum(id) => self.types.get(id).map_or_else(
                || Cow::Owned(id.to_string()),
                |entry| Cow::Borrowed(entry.display_name()),
            ),
        }
    }

    /// The registered disk codecs, as a handle a compiled program keeps.
    pub(crate) const fn codecs(&self) -> &Arc<Codecs> {
        &self.codecs
    }

    /// Add every func and type `other` holds, under the same gates
    /// [`add`](Self::add) and [`register_type`](Self::register_type) apply.
    pub fn merge(&mut self, other: Self) {
        for func in other.funcs.into_values() {
            self.add(func);
        }
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each registration is keyed, so the order changes nothing"
        )]
        for (type_id, entry) in other.types {
            self.register_type(type_id, entry);
        }
    }

    /// Whether an authored `Const` literal `value` may sit on `input`: the `Const` half of
    /// [`Graph::admit`](crate::Graph::admit) (the `Wire` half is
    /// [`DataType::compatible_with`]). The registry contributes only the enum arm, a variant
    /// *name*; the rest is [`FuncInput::accepts_const`].
    pub(crate) fn const_satisfies(&self, input: &FuncInput, value: &ConstValue) -> bool {
        input.accepts_const(value, Strictness::Authored, |type_id, name| {
            self.enum_variants(type_id)
                .is_some_and(|variants| variants.iter().any(|variant| variant == name))
        })
    }
}

/// Every type a `func` port declares as [`DataType::Enum`], inputs and
/// outputs alike.
fn declared_enum_types(func: &Func) -> impl Iterator<Item = TypeId> + '_ {
    let enum_id = |ty: &DataType| match ty {
        DataType::Enum(type_id) => Some(*type_id),
        _ => None,
    };
    let inputs = func
        .inputs()
        .iter()
        .filter_map(move |i| enum_id(&i.data_type));
    let outputs = func
        .outputs()
        .iter()
        .filter_map(move |o| enum_id(&o.data_type));
    inputs.chain(outputs)
}

/// Panic when `func` declares an `Enum` default whose name isn't among its type's registered
/// variants: the half of `FuncBuilder::build`'s checks that needs the registry. A default on a
/// type `variants_of` doesn't resolve passes; `register_type` re-checks when the entry arrives.
///
/// Variants arrive through `variants_of` rather than `&self` because [`Library::register_type`]
/// runs this *before* its entry is installed.
fn assert_enum_defaults<'a>(func: &Func, variants_of: impl Fn(TypeId) -> Option<&'a [String]>) {
    for input in func.inputs() {
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
                func.name(),
                input.name,
            );
        }
    }
}

impl<'a> IntoIterator for &'a Library {
    type Item = &'a Func;
    type IntoIter = Values<'a, FuncId, Func>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::graph::func::Func;
    use crate::graph::identity::FuncId;
    use crate::library::Library;

    impl Library {
        /// Take a func declaration out, for a fixture that edits it and adds it back.
        pub(crate) fn remove(&mut self, id: FuncId) -> Option<Func> {
            self.funcs.remove(&id)
        }
    }
}

#[cfg(test)]
mod tests;
