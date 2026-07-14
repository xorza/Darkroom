use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::graph::subgraph::{SubgraphDef, SubgraphId};
use crate::node::definition::{Func, FuncId};
use crate::{CustomValueCodec, ResourceStamper};
use crate::{DataType, EnumVariants, TypeId};
use common::KeyIndexVec;

/// The metadata of a registered nominal type — a `Custom`
/// app-extension type or an `Enum`. Identity is the [`TypeId`] it's keyed by in
/// [`Library::types`]; this is everything else the editor needs to render and
/// validate it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeDecl {
    Custom {
        display_name: String,
    },
    Enum {
        display_name: String,
        variants: Vec<String>,
    },
}

impl TypeDecl {
    pub fn display_name(&self) -> &str {
        match self {
            TypeDecl::Custom { display_name } | TypeDecl::Enum { display_name, .. } => display_name,
        }
    }

    /// The variant names for an `Enum` decl; `None` for a `Custom` type.
    pub fn variants(&self) -> Option<&[String]> {
        match self {
            TypeDecl::Enum { variants, .. } => Some(variants),
            TypeDecl::Custom { .. } => None,
        }
    }
}

/// A registered type declaration plus its optional runtime
/// attachments — the [`CustomValueCodec`] that makes its values disk-cacheable, and the
/// [`ResourceStamper`] that marks it a resource-reference type (its values name external
/// state whose identity folds into consumers' digests — see `execution/digest`). Both are
/// attachments. An `Enum` never carries either; enum values serialize directly as
/// [`crate::StaticValue`] inside authored graphs.
#[derive(Clone, Debug)]
pub struct TypeEntry {
    pub decl: TypeDecl,
    pub codec: Option<Arc<dyn CustomValueCodec>>,
    pub stamper: Option<Arc<dyn ResourceStamper>>,
}

impl TypeEntry {
    /// A custom type with no disk codec (not cacheable).
    pub fn custom(display_name: impl Into<String>) -> Self {
        Self {
            decl: TypeDecl::Custom {
                display_name: display_name.into(),
            },
            codec: None,
            stamper: None,
        }
    }

    /// A custom type with a disk codec.
    pub fn custom_with_codec(
        display_name: impl Into<String>,
        codec: Arc<dyn CustomValueCodec>,
    ) -> Self {
        Self {
            codec: Some(codec),
            ..Self::custom(display_name)
        }
    }

    /// Declare this a resource-reference type by attaching its [`ResourceStamper`].
    pub fn with_stamper(mut self, stamper: Arc<dyn ResourceStamper>) -> Self {
        self.stamper = Some(stamper);
        self
    }

    /// An enum type with the variant names taken from `E` (via strum).
    pub fn enum_of<E: EnumVariants>(display_name: impl Into<String>) -> Self {
        Self::enum_with_variants(display_name, E::variant_names())
    }

    /// An enum type with an explicit variant list (for runtime-discovered enums
    /// where the concrete type isn't available — see `lens`'s config builders).
    pub fn enum_with_variants(display_name: impl Into<String>, variants: Vec<String>) -> Self {
        Self {
            decl: TypeDecl::Enum {
                display_name: display_name.into(),
                variants,
            },
            codec: None,
            stamper: None,
        }
    }
}

/// The runtime registry every frontend resolves against: the [`Func`]s nodes
/// instantiate, the shared subgraph definitions, and the nominal types (with
/// their disk codecs). This is runtime registry state, not a persistence format;
/// authored graphs serialize function and type ids and resolve them against a
/// process-assembled library.
#[derive(Default, Debug, Clone)]
pub struct Library {
    pub funcs: KeyIndexVec<FuncId, Func>,

    /// Shared (linked) subgraph definitions. A node with
    /// `NodeKind::Subgraph(SubgraphRef::Linked(id))` resolves here; editing a
    /// def propagates to every linked instance. See `execution/README.md` Part A.
    pub subgraphs: KeyIndexVec<SubgraphId, SubgraphDef>,

    /// Registered nominal types (`Custom`/`Enum`), keyed by [`TypeId`]. The home
    /// for type metadata and the disk codecs the output cache dispatches through.
    /// Lookup-only (never iterated in order), so a plain map rather than a
    /// `KeyIndexVec`.
    pub types: HashMap<TypeId, TypeEntry>,
}

impl Library {
    pub fn by_id(&self, id: &FuncId) -> Option<&Func> {
        assert!(!id.is_nil());
        self.funcs.by_key(id)
    }
    pub fn by_name(&self, name: &str) -> Option<&Func> {
        assert!(!name.is_empty());
        self.funcs.iter().find(|func| func.name == name)
    }
    pub fn by_name_mut(&mut self, name: &str) -> Option<&mut Func> {
        assert!(!name.is_empty());
        self.funcs.iter_mut().find(|func| func.name == name)
    }
    pub fn add(&mut self, func: Func) {
        func.validate();

        self.funcs.add(func);
    }

    pub fn subgraph_by_id(&self, id: &SubgraphId) -> Option<&SubgraphDef> {
        assert!(!id.is_nil());
        self.subgraphs.by_key(id)
    }

    pub fn add_subgraph(&mut self, def: SubgraphDef) {
        assert!(!def.id.is_nil());
        self.subgraphs.add(def);
    }

    /// Register a nominal type. Panics on a duplicate id — two decls for one type
    /// is a wiring bug, not a runtime condition (as the old codec registry did).
    pub fn register_type(&mut self, type_id: impl Into<TypeId>, entry: TypeEntry) {
        let type_id = type_id.into();
        assert!(!type_id.is_nil());
        let prev = self.types.insert(type_id, entry);
        assert!(prev.is_none(), "duplicate type registration");
    }

    pub fn type_decl(&self, type_id: &TypeId) -> Option<&TypeDecl> {
        assert!(!type_id.is_nil());
        self.types.get(type_id).map(|entry| &entry.decl)
    }

    /// The variant names of a registered `Enum` type — for the editor's enum
    /// picker and the const type-check. `None` if `type_id` is unregistered or
    /// names a non-enum type.
    pub fn enum_variants(&self, type_id: &TypeId) -> Option<&[String]> {
        self.type_decl(type_id)?.variants()
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
                .type_decl(id)
                .map(|decl| Cow::Borrowed(decl.display_name()))
                .unwrap_or_else(|| Cow::Owned(id.to_string())),
        }
    }

    /// The disk codec registered for `type_id`, if any. Used by the output
    /// cache's serialize/deserialize.
    pub(crate) fn codec(&self, type_id: &TypeId) -> Option<&dyn CustomValueCodec> {
        self.types.get(type_id)?.codec.as_deref()
    }

    pub fn merge<T: Into<Library>>(&mut self, other: T) {
        let other = other.into();
        for func in other.funcs {
            self.add(func);
        }
        for def in other.subgraphs {
            self.add_subgraph(def);
        }
        for (type_id, entry) in other.types {
            self.register_type(type_id, entry);
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
mod tests {
    use crate::node::lambda::{InvokeInput, OutputUsage};
    use crate::runtime::any_state::AnyState;
    use crate::runtime::context::ContextManager;
    use crate::testing::{TestFuncHooks, test_func_lib};
    use crate::{DynamicValue, StaticValue};
    #[tokio::test]
    async fn invoke_by_id_and_index() -> anyhow::Result<()> {
        let library = test_func_lib(TestFuncHooks::default());
        let sum_id = library.by_name("sum").unwrap().id;

        let mut ctx_manager = ContextManager::default();
        let mut node_state = AnyState::default();
        let mut inputs = vec![
            InvokeInput {
                value: DynamicValue::Static(StaticValue::Int(2)),
            },
            InvokeInput {
                value: DynamicValue::Static(StaticValue::Int(4)),
            },
        ];
        let mut outputs = vec![DynamicValue::Unbound];
        let outputs_meta = vec![OutputUsage::Needed(1); outputs.len()];
        let event_state = crate::runtime::shared_any_state::SharedAnyState::default();
        library
            .by_id(&sum_id)
            .unwrap()
            .lambda
            .invoke(
                &mut ctx_manager,
                &mut node_state,
                &event_state,
                &mut inputs,
                &outputs_meta,
                &mut outputs,
            )
            .await?;
        assert_eq!(outputs[0].as_i64().unwrap(), 6);
        let cached = *node_state
            .get::<i64>()
            .expect("InvokeCache should contain the sum value");
        assert_eq!(cached, 6);

        inputs[0].value = DynamicValue::Static(StaticValue::Int(3));
        inputs[1].value = DynamicValue::Static(StaticValue::Int(5));
        outputs[0] = DynamicValue::Unbound;
        library
            .by_id(&sum_id)
            .unwrap()
            .lambda
            .invoke(
                &mut ctx_manager,
                &mut node_state,
                &event_state,
                &mut inputs,
                &outputs_meta,
                &mut outputs,
            )
            .await?;
        assert_eq!(outputs[0].as_i64().unwrap(), 8);

        Ok(())
    }
}
