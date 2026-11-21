//! [`LibraryBuilder`]: a library being composed, sealed into a [`Library`] by
//! [`build`](LibraryBuilder::build).

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::sync::Arc;

use common::Id;

use crate::data::data_type::custom_type::{CustomType, CustomTypeId};
use crate::data::data_type::enum_type::{EnumId, EnumType};
use crate::data::data_type::file_kind::{FileKind, FileKindId};
use crate::graph::func::Func;
use crate::graph::identity::FuncId;
use crate::library::Library;
use crate::library::func_redirect::{FuncRedirect, MAX_REDIRECTS};

/// The funcs and types a library is composed from, each fragment adding its own or merging
/// another's, for [`build`](Self::build) to check whole and seal; the library module's
/// decisions give the reason.
#[derive(Debug, Default)]
pub struct LibraryBuilder {
    pub(super) funcs: BTreeMap<FuncId, Func>,
    pub(super) enums: BTreeMap<EnumId, EnumType>,
    pub(super) custom_types: BTreeMap<CustomTypeId, CustomType>,
    pub(super) file_kinds: BTreeMap<FileKindId, FileKind>,
    pub(super) redirects: BTreeMap<FuncId, FuncRedirect>,
}

impl LibraryBuilder {
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
    /// replaces it; [`Self::build`] holds the chain to its rules.
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

    /// Register an enum. Registering an identical one again does nothing, so libraries that
    /// share it merge.
    ///
    /// # Panics
    /// If `id` is nil, or another enum is registered under it.
    pub fn add_enum(&mut self, id: EnumId, enum_type: EnumType) {
        register(&mut self.enums, id, enum_type, PartialEq::eq);
    }

    /// Register a custom type, with its codec if it has one. Registering an identical one again
    /// does nothing, so libraries that share it merge.
    ///
    /// # Panics
    /// If `id` is nil, or another custom type is registered under it: another name, or another
    /// codec instance.
    pub fn add_custom_type(&mut self, id: CustomTypeId, custom_type: CustomType) {
        register(&mut self.custom_types, id, custom_type, CustomType::same_as);
    }

    /// Register a file kind. Registering an identical one again does nothing, so libraries that
    /// share it merge.
    ///
    /// # Panics
    /// If `id` is nil, or another file kind is registered under it.
    pub fn add_file_kind(&mut self, id: FileKindId, file_kind: FileKind) {
        register(&mut self.file_kinds, id, file_kind, PartialEq::eq);
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
        } = other;
        for func in funcs.into_values() {
            self.add(func);
        }
        for redirect in redirects.into_values() {
            self.add_redirect(redirect);
        }
        for (id, enum_type) in enums {
            self.add_enum(id, enum_type);
        }
        for (id, custom_type) in custom_types {
            self.add_custom_type(id, custom_type);
        }
        for (id, file_kind) in file_kinds {
            self.add_file_kind(id, file_kind);
        }
    }

    /// The library this composes, checked whole: every redirect from an unregistered func ends
    /// at a registered one within [`MAX_REDIRECTS`], its chain composed once; every type a
    /// func's port names is registered; every enum default a func declares is a variant of its
    /// enum.
    ///
    /// # Panics
    /// On the first redirect or func that breaks a rule, naming it: a library is composed by
    /// code, so a gap is a bug in that code.
    pub fn build(self) -> Library {
        let library = self.assemble();
        library.check();
        library
    }

    /// The library this composes, its redirects flattened, its codecs and its type list derived,
    /// its types unchecked.
    ///
    /// # Panics
    /// On the first redirect, in id order, from a registered func or whose chain loops, runs past
    /// [`MAX_REDIRECTS`] or ends at no registered func.
    pub(super) fn assemble(self) -> Library {
        let redirects = self
            .redirects
            .keys()
            .map(|&from| (from, self.land(from)))
            .collect();
        let codecs = self
            .custom_types
            .iter()
            .filter_map(|(&id, custom_type)| Some((id, Arc::clone(custom_type.codec()?))))
            .collect();
        let mut library = Library {
            funcs: self.funcs,
            enums: self.enums,
            custom_types: self.custom_types,
            file_kinds: self.file_kinds,
            redirects,
            data_types: Vec::new(),
            codecs: Arc::new(codecs),
        };
        library.data_types = library.sorted_data_types();
        library
    }

    /// The func a node of `from` lands on through the redirects, with the one remap of the whole
    /// chain.
    ///
    /// # Panics
    /// If `from` is registered, or its chain loops, runs past [`MAX_REDIRECTS`] or ends at no
    /// registered func.
    fn land(&self, from: FuncId) -> FuncRedirect {
        assert!(
            !self.funcs.contains_key(&from),
            "a redirect from {from:?} names a func the library registers"
        );
        let mut landed = self.redirects[&from].clone();
        for _ in 0..MAX_REDIRECTS {
            if self.funcs.contains_key(&landed.to) {
                return landed;
            }
            let Some(next) = self.redirects.get(&landed.to) else {
                break;
            };
            landed = FuncRedirect {
                from,
                to: next.to,
                remap: landed.remap.then(&next.remap),
            };
        }
        panic!(
            "the redirects from {from:?} loop, run past {MAX_REDIRECTS}, or end at no registered \
             func"
        );
    }
}

/// Insert `entry` under `id` into `table`, or do nothing when `same` finds the one held there
/// identical.
///
/// # Panics
/// If `id` is nil, or the table holds another entry under it.
fn register<M, T>(
    table: &mut BTreeMap<Id<M>, T>,
    id: Id<M>,
    entry: T,
    same: impl FnOnce(&T, &T) -> bool,
) {
    assert!(!id.is_nil(), "cannot register a type under the nil id");
    match table.entry(id) {
        Entry::Occupied(held) => {
            assert!(
                same(held.get(), &entry),
                "conflicting registration of type {id:?}"
            );
        }
        Entry::Vacant(slot) => {
            slot.insert(entry);
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::library::library_builder::LibraryBuilder;

    /// What a test counts of a fragment, which may not build alone: a fragment can name a type
    /// another registers.
    impl LibraryBuilder {
        pub fn func_count(&self) -> usize {
            self.funcs.len()
        }

        /// How many types the fragment registers, of every kind.
        pub fn type_count(&self) -> usize {
            self.enums.len() + self.custom_types.len() + self.file_kinds.len()
        }
    }
}
