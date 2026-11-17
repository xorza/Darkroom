//! [`Library`]: the funcs nodes instantiate and the types their ports name.
//!
//! # Decisions
//!
//! - **A library registers every type its funcs declare**, held by [`Library::check`] once it is
//!   composed. So a type the library lacks is one no func here declares: a document names it on
//!   an interface port alone, and no body reads a value of it. Refused: a check at each `add` and
//!   each registration of the other's entries, as the two arrive in either order.
//! - **A type the library lacks is a type like any other, equal only to itself.** A newer
//!   version, a build without a feature or a shared compound can name one this build does not
//!   register; the port, its bindings and its constants stay as written. A constant of an
//!   unknown enum is kept unchecked, an unknown file kind is within itself alone, and an unknown
//!   custom type has no disk codec. The same promise a ghost node makes for a node with no func: kept,
//!   inert, shown with its reason, live again when the library has it.

pub(crate) mod func_redirect;

use std::collections::HashMap;
use std::collections::hash_map::{Entry, Values};
use std::sync::Arc;

use common::Id;

use crate::data::codec::Codecs;
use crate::data::const_value::ConstValue;
use crate::data::data_type::custom_type::{CustomType, CustomTypeId};
use crate::data::data_type::enum_type::{EnumId, EnumType};
use crate::data::data_type::file_kind::{FileKind, FileKindId};
use crate::data::data_type::{DataType, Strictness};
use crate::graph::error::BindingRejection;
use crate::graph::func::Func;
use crate::graph::identity::FuncId;
use crate::graph::ports::{InputDecl, OutputDecl};
use crate::graph::type_names::TypeRef;
use crate::library::func_redirect::{FuncRedirect, MAX_REDIRECTS};

/// The name a type shows when the library does not register it.
pub(crate) const UNKNOWN_TYPE: &str = "unknown type";

/// The runtime registry every frontend resolves against: the [`Func`]s nodes instantiate, and the
/// types their ports name, each kind in its own table under its own id type. Not a persistence
/// format: authored graphs serialize func and type ids and resolve them against a
/// process-assembled library.
#[derive(Default, Debug)]
pub struct Library {
    funcs: HashMap<FuncId, Func>,
    enums: HashMap<EnumId, EnumType>,
    custom_types: HashMap<CustomTypeId, CustomType>,
    file_kinds: HashMap<FileKindId, FileKind>,
    /// The funcs the library no longer registers, each by the func that replaces it.
    redirects: HashMap<FuncId, FuncRedirect>,
    /// The codecs among `custom_types`, shared with each program compiled from this library;
    /// registration copies the map only while a program still holds the previous one.
    codecs: Arc<Codecs>,
}

impl Library {
    /// The func with `id`, or `None`, as for the nil id, which
    /// [`FuncBuilder::build`](crate::FuncBuilder::build) refuses.
    pub fn by_id(&self, id: FuncId) -> Option<&Func> {
        self.funcs.get(&id)
    }

    pub fn by_name(&self, name: &str) -> Option<&Func> {
        self.iter().find(|func| func.name() == name)
    }

    /// The registered funcs, in no particular order.
    pub fn iter(&self) -> Values<'_, FuncId, Func> {
        self.funcs.values()
    }

    /// # Panics
    /// If a func is registered under its id already.
    pub fn add(&mut self, func: Func) {
        assert!(
            !self.funcs.contains_key(&func.id()),
            "duplicate function registration"
        );
        self.funcs.insert(func.id(), func);
    }

    /// Register `redirect`, from a func id this library does not register to the func that
    /// replaces it; [`Self::check`] holds the chain to its rules.
    ///
    /// # Panics
    /// If a redirect from the same id is registered already.
    pub fn add_redirect(&mut self, redirect: FuncRedirect) {
        assert!(
            !self.redirects.contains_key(&redirect.from),
            "duplicate redirect registration"
        );
        self.redirects.insert(redirect.from, redirect);
    }

    /// Where a node of func `from`, which this library does not register, goes: the func at the
    /// end of its redirects, and the one remap of them all. `None` for a registered func or one
    /// no redirect names.
    pub fn redirect(&self, from: FuncId) -> Option<FuncRedirect> {
        if self.funcs.contains_key(&from) {
            return None;
        }
        let first = self.redirects.get(&from)?;
        let mut landed = first.clone();
        for _ in 0..MAX_REDIRECTS {
            if self.funcs.contains_key(&landed.to) {
                return Some(landed);
            }
            let next = self.redirects.get(&landed.to)?;
            landed = FuncRedirect {
                from,
                to: next.to,
                remap: landed.remap.then(&next.remap),
            };
        }
        None
    }

    /// Register an enum. Registering an identical one again does nothing, so libraries that
    /// share it merge.
    ///
    /// # Panics
    /// If `id` is nil, or another enum is registered under it.
    pub fn register_enum(&mut self, id: EnumId, enum_type: EnumType) {
        register(&mut self.enums, id, enum_type, PartialEq::eq);
    }

    /// Register a custom type, and its codec if it has one. Registering an identical one again
    /// does nothing, so libraries that share it merge.
    ///
    /// # Panics
    /// If `id` is nil, or another custom type is registered under it: another name, or another
    /// codec instance.
    pub fn register_custom_type(&mut self, id: CustomTypeId, custom_type: CustomType) {
        let codec = custom_type.codec().map(Arc::clone);
        if register(&mut self.custom_types, id, custom_type, CustomType::same_as)
            && let Some(codec) = codec
        {
            Arc::make_mut(&mut self.codecs).insert(id, codec);
        }
    }

    /// Register a file kind. Registering an identical one again does nothing, so libraries that
    /// share it merge.
    ///
    /// # Panics
    /// If `id` is nil, or another file kind is registered under it.
    pub fn register_file_kind(&mut self, id: FileKindId, file_kind: FileKind) {
        register(&mut self.file_kinds, id, file_kind, PartialEq::eq);
    }

    pub fn enum_type(&self, id: EnumId) -> Option<&EnumType> {
        self.enums.get(&id)
    }

    pub fn custom_type(&self, id: CustomTypeId) -> Option<&CustomType> {
        self.custom_types.get(&id)
    }

    pub fn file_kind(&self, id: FileKindId) -> Option<&FileKind> {
        self.file_kinds.get(&id)
    }

    /// Each custom and enum type the library registers, ordered by name, then by id: what a port
    /// can be retyped to beside the scalars. A file kind is left out, as its port also names how
    /// it is taken.
    pub fn data_types(&self) -> Vec<DataType> {
        let mut types: Vec<DataType> = self
            .custom_types
            .keys()
            .map(|&id| DataType::Custom(id))
            .chain(self.enums.keys().map(|&id| DataType::Enum(id)))
            .collect();
        let id = |data_type: DataType| match data_type {
            DataType::Custom(id) => id.as_u128(),
            DataType::Enum(id) => id.as_u128(),
            _ => unreachable!("the list holds custom and enum types"),
        };
        types.sort_by(|&a, &b| {
            self.type_name(a)
                .cmp(self.type_name(b))
                .then_with(|| id(a).cmp(&id(b)))
        });
        types
    }

    /// How many types the library registers, of every kind.
    pub fn type_count(&self) -> usize {
        self.enums.len() + self.custom_types.len() + self.file_kinds.len()
    }

    /// A short human-readable name for `data_type`: the scalar keyword, `"directory"`, or a
    /// registered type's name, which is "unknown type" for one the library does not register.
    pub fn type_name(&self, data_type: DataType) -> &str {
        match data_type {
            DataType::Any => "any",
            DataType::Float => "float",
            DataType::Int => "int",
            DataType::Bool => "bool",
            DataType::String => "string",
            DataType::Dir => "directory",
            DataType::File { .. } | DataType::Custom(_) | DataType::Enum(_) => {
                TypeRef::of(data_type)
                    .and_then(|type_ref| self.registered_name(type_ref))
                    .unwrap_or(UNKNOWN_TYPE)
            }
        }
    }

    /// Whether the library registers `type_ref`.
    pub fn knows_type(&self, type_ref: TypeRef) -> bool {
        match type_ref {
            TypeRef::FileKind(kind) => self.file_kinds.contains_key(&kind),
            TypeRef::Custom(id) => self.custom_types.contains_key(&id),
            TypeRef::Enum(id) => self.enums.contains_key(&id),
        }
    }

    /// The name the library registers `type_ref` under.
    pub fn registered_name(&self, type_ref: TypeRef) -> Option<&str> {
        match type_ref {
            TypeRef::FileKind(kind) => self.file_kind(kind).map(FileKind::name),
            TypeRef::Custom(id) => self.custom_type(id).map(CustomType::name),
            TypeRef::Enum(id) => self.enum_type(id).map(EnumType::name),
        }
    }

    /// Whether the library registers every type `data_type` names: none for a scalar.
    pub fn knows(&self, data_type: DataType) -> bool {
        TypeRef::of(data_type).is_none_or(|type_ref| self.knows_type(type_ref))
    }

    /// The registered disk codecs, as a handle a compiled program keeps.
    pub(crate) const fn codecs(&self) -> &Arc<Codecs> {
        &self.codecs
    }

    /// Add every func and type `other` holds, under the rules [`add`](Self::add) and each
    /// registration apply.
    pub fn merge(&mut self, other: Self) {
        let Self {
            funcs,
            enums,
            custom_types,
            file_kinds,
            redirects,
            codecs: _,
        } = other;
        for func in funcs.into_values() {
            self.add(func);
        }
        for redirect in redirects.into_values() {
            self.add_redirect(redirect);
        }
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each registration is keyed, so the order changes nothing"
        )]
        for (id, enum_type) in enums {
            self.register_enum(id, enum_type);
        }
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each registration is keyed, so the order changes nothing"
        )]
        for (id, custom_type) in custom_types {
            self.register_custom_type(id, custom_type);
        }
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each registration is keyed, so the order changes nothing"
        )]
        for (id, file_kind) in file_kinds {
            self.register_file_kind(id, file_kind);
        }
    }

    /// Check the library is whole, once it is composed: every type a func's port names is
    /// registered, and every enum default a func declares is a variant of its enum.
    ///
    /// # Panics
    /// On the first func that breaks either rule, naming the func and the port: a library is
    /// composed by code, so a gap is a bug in that code.
    pub fn check(&self) {
        let mut redirects: Vec<&FuncRedirect> = self.redirects.values().collect();
        redirects.sort_unstable_by_key(|redirect| redirect.from);
        for redirect in redirects {
            assert!(
                !self.funcs.contains_key(&redirect.from),
                "a redirect from {:?} names a func the library registers",
                redirect.from
            );
            assert!(
                self.redirect(redirect.from).is_some(),
                "the redirects from {:?} loop, run past {MAX_REDIRECTS}, or end at no registered \
                 func",
                redirect.from
            );
        }
        let mut funcs: Vec<&Func> = self.funcs.values().collect();
        funcs.sort_unstable_by_key(|func| func.id());
        for func in funcs {
            let types = func
                .inputs()
                .iter()
                .map(|input| (&input.name, input.data_type))
                .chain(
                    func.outputs()
                        .iter()
                        .map(|output| (&output.name, output.data_type)),
                );
            for (port, data_type) in types {
                assert!(
                    self.knows(data_type),
                    "function {:?} port {port:?} names {data_type:?}, which the library does not \
                     register",
                    func.name(),
                );
            }
            for input in func.inputs() {
                let (DataType::Enum(enum_id), Some(ConstValue::Enum(name)), true) = (
                    input.data_type,
                    &input.default_value,
                    input.domain.variants().is_empty(),
                ) else {
                    continue;
                };
                assert!(
                    self.enum_type(enum_id)
                        .is_some_and(|enum_type| enum_type.has_variant(name)),
                    "function {:?} input {:?} defaults to {name:?}, which is not a variant of \
                     its enum",
                    func.name(),
                    input.name,
                );
            }
        }
    }

    /// Whether an authored `Const` literal `value` may sit on `input`: the `Const` half of
    /// [`Graph::admit`](crate::Graph::admit), [`Self::admit_wire`] the `Wire` half. The
    /// registry contributes only the enum arm, a variant *name*, which a constant of an enum the
    /// library does not register keeps unchecked; the rest is [`InputDecl::accepts_const`].
    pub(crate) fn const_satisfies(&self, input: &InputDecl, value: &ConstValue) -> bool {
        input.accepts_const(value, Strictness::Authored, |enum_id, name| {
            self.enum_type(enum_id)
                .is_none_or(|enum_type| enum_type.has_variant(name))
        })
    }

    /// Whether a wire from `output`, which holds only a constant when `output_is_const`, may
    /// feed `input`: of a type the input accepts, into an input that is not const-only unless
    /// the output holds a constant. The one rule [`Graph::admit_onto`](crate::Graph::admit_onto)
    /// holds a wire to, which an editor also asks of a node it has not placed yet.
    ///
    /// # Errors
    ///
    /// [`BindingRejection::WiredConstOnly`] for a const-only input fed by an output that holds
    /// more than a constant, and [`BindingRejection::WireMismatch`] for a type the input does not
    /// accept.
    pub fn admit_wire(
        &self,
        input: &InputDecl,
        output: &OutputDecl,
        output_is_const: bool,
    ) -> Result<(), BindingRejection> {
        if input.const_only && !output_is_const {
            return Err(BindingRejection::WiredConstOnly);
        }
        if self.wire_satisfies(input.data_type, output.data_type) {
            Ok(())
        } else {
            Err(BindingRejection::WireMismatch)
        }
    }

    /// Whether an output of type `source` may feed an input of type `input`: the type half of
    /// [`Self::admit_wire`]. The registry contributes the file arm, whether the source's file
    /// kind is within the input's; the rest is [`DataType::compatible_with`].
    pub(crate) fn wire_satisfies(&self, input: DataType, source: DataType) -> bool {
        input.compatible_with(source, |source, input| self.file_kind_within(source, input))
    }

    /// Whether every file the kind `source` holds, the kind `input` holds: a kind is within
    /// itself, and a kind the library does not register within nothing else.
    fn file_kind_within(&self, source: FileKindId, input: FileKindId) -> bool {
        source == input
            || self
                .file_kind(source)
                .zip(self.file_kind(input))
                .is_some_and(|(source, input)| source.within(input))
    }
}

/// Insert `entry` under `id` into `table`, or do nothing when `same` finds the one held there
/// identical. Whether it inserted.
///
/// # Panics
/// If `id` is nil, or the table holds another entry under it.
fn register<M, T>(
    table: &mut HashMap<Id<M>, T>,
    id: Id<M>,
    entry: T,
    same: impl FnOnce(&T, &T) -> bool,
) -> bool {
    assert!(!id.is_nil(), "cannot register a type under the nil id");
    match table.entry(id) {
        Entry::Occupied(held) => {
            assert!(
                same(held.get(), &entry),
                "conflicting registration of type {id:?}"
            );
            false
        }
        Entry::Vacant(slot) => {
            slot.insert(entry);
            true
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
