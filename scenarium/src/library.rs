use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::graph::Graph;
use crate::graph::interface::GraphId;
use crate::node::definition::{Func, FuncId};
use crate::{CustomValueCodec, ResourceStamper};
use crate::{DataType, EnumVariants, TypeId};
use hashbrown::HashMap as GraphMap;
use thiserror::Error;

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

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TypeEntryValidationError {
    #[error("enum type cannot have a codec")]
    EnumCodec,
    #[error("enum type cannot have a stamper")]
    EnumStamper,
}

impl TypeEntry {
    /// Validates that this declaration's runtime attachments match its kind.
    pub fn validate(&self) -> Result<(), TypeEntryValidationError> {
        if matches!(&self.decl, TypeDecl::Enum { .. }) {
            if self.codec.is_some() {
                return Err(TypeEntryValidationError::EnumCodec);
            }
            if self.stamper.is_some() {
                return Err(TypeEntryValidationError::EnumStamper);
            }
        }
        Ok(())
    }

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
/// instantiate, the shared graphs, and the nominal types (with
/// their disk codecs). This is runtime registry state, not a persistence format;
/// authored graphs serialize function and type ids and resolve them against a
/// process-assembled library.
#[derive(Default, Debug, Clone)]
pub struct Library {
    funcs: HashMap<FuncId, Func>,

    /// Shared graphs. Editing one propagates to every shared instance.
    pub graphs: GraphMap<GraphId, Graph>,

    /// Registered nominal types (`Custom`/`Enum`), keyed by [`TypeId`]. The home
    /// for type metadata and the disk codecs the output cache dispatches through.
    /// Lookup-only (never iterated in order), so a plain map rather than an
    /// ordered map.
    pub types: HashMap<TypeId, TypeEntry>,
}

impl Library {
    pub fn by_id(&self, id: &FuncId) -> Option<&Func> {
        assert!(!id.is_nil());
        self.funcs.get(id)
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
        self.funcs.insert(func.id, func);
    }

    pub fn remove(&mut self, id: &FuncId) -> Option<Func> {
        self.funcs.remove(id)
    }

    pub fn graph_by_id(&self, id: &GraphId) -> Option<&Graph> {
        assert!(!id.is_nil());
        self.graphs.get(id)
    }

    /// Inserts a shared graph, replacing the graph with the same id.
    pub fn insert_graph(&mut self, id: GraphId, graph: Graph) {
        assert!(!id.is_nil());
        self.graphs.insert(id, graph);
    }

    /// Register a nominal type. Panics on a duplicate id — two decls for one type
    /// is a wiring bug, not a runtime condition (as the old codec registry did).
    pub fn register_type(&mut self, type_id: impl Into<TypeId>, entry: TypeEntry) {
        let type_id = type_id.into();
        assert!(!type_id.is_nil());
        entry.validate().expect("invalid type declaration");
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
        for func in other.funcs.into_values() {
            self.add(func);
        }
        for (id, graph) in other.graphs {
            self.insert_graph(id, graph);
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
    use std::sync::Arc;

    use tokio::io::{AsyncRead, AsyncWrite};

    use crate::graph::Graph;
    use crate::graph::interface::GraphId;
    use crate::library::{Library, TypeEntry};
    use crate::node::definition::{Func, FuncId, FuncInput};
    use crate::node::lambda::{InvokeError, InvokeInput, OutputDemand};
    use crate::runtime::any_state::AnyState;
    use crate::runtime::context::ContextManager;
    use crate::runtime::shared_any_state::SharedAnyState;
    use crate::testing::{TestFuncHooks, test_func_lib};
    use crate::{
        CancelToken, CodecError, CustomValue, CustomValueCodec, DataType, DynamicValue,
        ResourceStamp, ResourceStamper, StaticValue, TypeId,
    };

    #[derive(Debug)]
    struct StubCodec;

    #[async_trait::async_trait]
    impl CustomValueCodec for StubCodec {
        fn version(&self) -> u32 {
            0
        }

        async fn encode(
            &self,
            _value: &dyn CustomValue,
            _writer: &mut (dyn AsyncWrite + Unpin + Send),
            _ctx: &mut ContextManager,
        ) -> std::result::Result<(), CodecError> {
            unreachable!()
        }

        async fn decode(
            &self,
            _reader: &mut (dyn AsyncRead + Unpin + Send),
            _byte_len: u64,
        ) -> std::result::Result<Arc<dyn CustomValue>, CodecError> {
            unreachable!()
        }
    }

    #[derive(Debug)]
    struct StubStamper;

    impl ResourceStamper for StubStamper {
        fn stamp(&self, _value: &DynamicValue, _cancel: &CancelToken) -> ResourceStamp {
            ResourceStamp::default()
        }
    }

    #[test]
    fn insert_graph_replaces_definition_with_same_id() {
        let id = GraphId::unique();
        let mut library = Library::default();

        library.insert_graph(id, Graph::new("Before"));
        assert_eq!(library.graphs.len(), 1);
        assert_eq!(library.graph_by_id(&id).unwrap().name, "Before");

        library.insert_graph(id, Graph::new("After"));
        assert_eq!(library.graphs.len(), 1);
        assert_eq!(library.graph_by_id(&id).unwrap().name, "After");
    }

    #[test]
    fn add_rejects_invalid_function_declarations() {
        for func in [
            Func::new(FuncId::nil(), "nil"),
            Func::new(FuncId::unique(), "wildcard")
                .input(FuncInput::required("value", DataType::Any))
                .wildcard_output("value", 1),
        ] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Library::default().add(func);
            }));
            assert!(result.is_err(), "invalid declaration was registered");
        }
    }

    #[test]
    fn register_type_rejects_enum_runtime_attachments() {
        let custom = TypeEntry::custom_with_codec("Custom", Arc::new(StubCodec))
            .with_stamper(Arc::new(StubStamper));
        custom.validate().unwrap();
        let mut library = Library::default();
        let custom_id = TypeId::unique();
        library.register_type(custom_id, custom);
        assert!(library.types[&custom_id].codec.is_some());
        assert!(library.types[&custom_id].stamper.is_some());

        let mut enum_with_codec = TypeEntry::enum_with_variants("Enum", vec!["A".into()]);
        enum_with_codec.codec = Some(Arc::new(StubCodec));
        let enum_with_stamper = TypeEntry::enum_with_variants("Enum", vec!["A".into()])
            .with_stamper(Arc::new(StubStamper));
        for (expected, entry) in [
            ("enum type cannot have a codec", enum_with_codec),
            ("enum type cannot have a stamper", enum_with_stamper),
        ] {
            assert_eq!(entry.validate().unwrap_err().to_string(), expected);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Library::default().register_type(TypeId::unique(), entry);
            }));
            assert!(
                result.is_err(),
                "invalid declaration was registered: {expected}"
            );
        }
    }

    #[tokio::test]
    async fn invoke_by_id_and_index() -> Result<(), InvokeError> {
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
        let output_demand = vec![OutputDemand::Produce; outputs.len()];
        let event_state = SharedAnyState::default();
        library
            .by_id(&sum_id)
            .unwrap()
            .lambda
            .invoke(
                &mut ctx_manager,
                &mut node_state,
                &event_state,
                &mut inputs,
                &output_demand,
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
                &output_demand,
                &mut outputs,
            )
            .await?;
        assert_eq!(outputs[0].as_i64().unwrap(), 8);

        Ok(())
    }
}
