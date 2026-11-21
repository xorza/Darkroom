//! [`Library`]: the funcs nodes instantiate and the types their ports name.
//!
//! # Decisions
//!
//! - **A library is composed, then built whole and read.** A [`LibraryBuilder`] takes each
//!   fragment's funcs and types, and [`LibraryBuilder::build`] checks the whole once and derives
//!   what readers ask for: each redirect chain flattened, the type list sorted, the codec table.
//!   So no reader meets a library half-composed, no caller has to remember the check, and nothing
//!   is derived again per call, as the type list was each frame. The builder pattern of
//!   [C-BUILDER](https://rust-lang.github.io/api-guidelines/type-safety.html#c-builder), as
//!   `std::process::Command` builds a process. Refused: mutators on `Library` beside a `check`
//!   each composer calls.
//! - **Each table is a `BTreeMap` by id**, so every walk of it, the first gap a build names and
//!   the ties of the type list among them, is the same in every process. Refused: `HashMap`,
//!   whose order is seeded per process; a lookup among the hundred-odd funcs costs a handful of
//!   comparisons.
//! - **A library registers every type its funcs declare**, held by the build. So a type the
//!   library lacks is one no func here declares: a document names it on an interface port alone,
//!   and no body reads a value of it. Refused: a check at each `add` and each registration of the
//!   other's entries, as the two arrive in either order.
//! - **A type the library lacks is a type like any other, equal only to itself.** A newer
//!   version, a build without a feature or a shared compound can name one this build does not
//!   register; the port, its bindings and its constants stay as written. A constant of an
//!   unknown enum is kept unchecked, an unknown file kind is within itself alone, and an unknown
//!   custom type has no disk codec. The same promise a ghost node makes for a node with no func:
//!   kept, inert, shown with its reason, live again when the library has it.
//!
//! [`LibraryBuilder`]: crate::LibraryBuilder
//! [`LibraryBuilder::build`]: crate::LibraryBuilder::build

pub(crate) mod func_redirect;
pub(crate) mod library_builder;

use std::collections::BTreeMap;
use std::collections::btree_map::Values;
use std::sync::Arc;

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
use crate::library::func_redirect::FuncRedirect;

/// The name a type shows when the library does not register it.
pub(crate) const UNKNOWN_TYPE: &str = "Unknown Type";

/// The runtime registry every frontend resolves against: the [`Func`]s nodes instantiate, and the
/// types their ports name, each kind in its own table under its own id type. Not a persistence
/// format: authored graphs serialize func and type ids and resolve them against a
/// process-assembled library.
#[derive(Default, Debug)]
pub struct Library {
    funcs: BTreeMap<FuncId, Func>,
    enums: BTreeMap<EnumId, EnumType>,
    custom_types: BTreeMap<CustomTypeId, CustomType>,
    file_kinds: BTreeMap<FileKindId, FileKind>,
    /// The funcs the library no longer registers, each by the registered func its redirects end
    /// at, with the one remap of the whole chain.
    redirects: BTreeMap<FuncId, FuncRedirect>,
    /// The custom and enum types by name, then id: [`Self::data_types`].
    data_types: Vec<DataType>,
    /// The codecs among `custom_types`, shared with each program compiled from this library.
    codecs: Arc<Codecs>,
}

impl Library {
    /// The func with `id`, or `None`, as for the nil id, which
    /// [`FuncBuilder::build`](crate::FuncBuilder::build) refuses.
    pub fn by_id(&self, id: FuncId) -> Option<&Func> {
        self.funcs.get(&id)
    }

    /// The registered funcs, in id order.
    pub fn iter(&self) -> Values<'_, FuncId, Func> {
        self.funcs.values()
    }

    /// Where a node of func `from`, which this library does not register, goes: the func at the
    /// end of its redirects, and the one remap of them all, composed at the build. `None` for a
    /// registered func or one no redirect names.
    pub fn redirect(&self, from: FuncId) -> Option<&FuncRedirect> {
        self.redirects.get(&from)
    }

    pub fn enum_type(&self, id: EnumId) -> Option<&EnumType> {
        self.enums.get(&id)
    }

    pub fn file_kind(&self, id: FileKindId) -> Option<&FileKind> {
        self.file_kinds.get(&id)
    }

    /// Each custom and enum type the library registers, ordered by name, then by id: what a port
    /// can be retyped to beside the scalars. A file kind is left out, as its port also names how
    /// it is taken. Sorted once, at the build.
    pub fn data_types(&self) -> &[DataType] {
        &self.data_types
    }

    fn sorted_data_types(&self) -> Vec<DataType> {
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

    /// The name a person reads for `data_type`, in Title Case as every type name is: a built-in's
    /// word, as Blender names its sockets (`Integer`, `Boolean`), or a registered type's name,
    /// which is "Unknown Type" for one the library does not register.
    pub fn type_name(&self, data_type: DataType) -> &str {
        match data_type {
            DataType::Any => "Any",
            DataType::Float => "Float",
            DataType::Int => "Integer",
            DataType::Bool => "Boolean",
            DataType::String => "String",
            DataType::Dir => "Directory",
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
            TypeRef::Custom(id) => self.custom_types.get(&id).map(CustomType::name),
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

    /// Check every type a func's port names is registered, and every enum default a func
    /// declares is a variant of its enum: what [`LibraryBuilder::build`] asserts once the
    /// registry can answer.
    ///
    /// # Panics
    /// On the first func that breaks a rule, in id order, naming the func and the port.
    ///
    /// [`LibraryBuilder::build`]: crate::LibraryBuilder::build
    fn check(&self) {
        for func in self.funcs.values() {
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

    /// The constant a port of `data_type` starts with where its declaration states none: an
    /// enum's first variant, an empty text for `Any`, which an editor reads the kind off as it is
    /// typed, a scalar's zero, an empty path. `None` for a custom type, which has no constant, and
    /// for an enum the library lacks or that holds no variant.
    pub fn type_default(&self, data_type: DataType) -> Option<ConstValue> {
        match data_type {
            DataType::Enum(id) => self
                .enum_type(id)?
                .variants()
                .first()
                .map(|first| ConstValue::Enum(first.clone())),
            DataType::Any => Some(ConstValue::String(String::new())),
            data_type => data_type.default_value(),
        }
    }

    /// Whether [`Self::type_default`] is some, answered without building it, for a reader that
    /// asks every frame.
    pub fn has_type_default(&self, data_type: DataType) -> bool {
        match data_type {
            DataType::Enum(id) => self
                .enum_type(id)
                .is_some_and(|enum_type| !enum_type.variants().is_empty()),
            DataType::Any => true,
            data_type => data_type.default_value().is_some(),
        }
    }

    /// The constant `input` starts with: its declared default, else its type's.
    pub fn default_const(&self, input: &InputDecl) -> Option<ConstValue> {
        input
            .default_value
            .clone()
            .or_else(|| self.type_default(input.data_type))
    }

    /// Whether `input` can hold a constant: [`Self::default_const`] is some, answered without
    /// building it.
    pub fn takes_const(&self, input: &InputDecl) -> bool {
        input.default_value.is_some() || self.has_type_default(input.data_type)
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

impl<'a> IntoIterator for &'a Library {
    type Item = &'a Func;
    type IntoIter = Values<'a, FuncId, Func>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use std::mem;

    use crate::data::data_type::custom_type::{CustomType, CustomTypeId};
    use crate::data::data_type::enum_type::{EnumId, EnumType};
    use crate::data::data_type::file_kind::{FileKind, FileKindId};
    use crate::graph::func::Func;
    use crate::graph::identity::FuncId;
    use crate::library::Library;
    use crate::library::library_builder::LibraryBuilder;

    /// A test's edits of a library in place, each sugar over a [`LibraryBuilder`] assembled
    /// again with its types unchecked: a fixture grows the library its graph resolves against
    /// as a test goes, and a test whose subject is the check builds through
    /// [`LibraryBuilder::build`].
    impl Library {
        /// This library as a builder again, for an edit to build anew.
        pub fn into_builder(self) -> LibraryBuilder {
            LibraryBuilder {
                funcs: self.funcs,
                enums: self.enums,
                custom_types: self.custom_types,
                file_kinds: self.file_kinds,
                redirects: self.redirects,
            }
        }

        fn edit<T>(&mut self, edit: impl FnOnce(&mut LibraryBuilder) -> T) -> T {
            let mut builder = mem::take(self).into_builder();
            let answer = edit(&mut builder);
            *self = builder.assemble();
            answer
        }

        /// [`LibraryBuilder::add`] in place.
        pub fn add(&mut self, func: Func) {
            self.edit(|builder| builder.add(func));
        }

        /// Take the func `id` out, as a library that dropped it.
        pub fn remove(&mut self, id: FuncId) -> Option<Func> {
            self.edit(|builder| builder.funcs.remove(&id))
        }

        /// [`LibraryBuilder::add_enum`] in place.
        pub fn add_enum(&mut self, id: EnumId, enum_type: EnumType) {
            self.edit(|builder| builder.add_enum(id, enum_type));
        }

        /// [`LibraryBuilder::add_custom_type`] in place.
        pub fn add_custom_type(&mut self, id: CustomTypeId, custom_type: CustomType) {
            self.edit(|builder| builder.add_custom_type(id, custom_type));
        }

        /// [`LibraryBuilder::add_file_kind`] in place.
        pub fn add_file_kind(&mut self, id: FileKindId, file_kind: FileKind) {
            self.edit(|builder| builder.add_file_kind(id, file_kind));
        }

        /// The func named `name`, the first in id order.
        pub fn by_name(&self, name: &str) -> Option<&Func> {
            self.iter().find(|func| func.name() == name)
        }

        /// How many types the library registers, of every kind.
        pub fn type_count(&self) -> usize {
            self.enums.len() + self.custom_types.len() + self.file_kinds.len()
        }

        pub fn custom_type(&self, id: CustomTypeId) -> Option<&CustomType> {
            self.custom_types.get(&id)
        }
    }
}

#[cfg(test)]
mod tests;
