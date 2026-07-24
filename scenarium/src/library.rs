use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::CustomValueCodec;
use crate::graph::Graph;
use crate::graph::interface::GraphId;
use crate::node::definition::{Func, FuncId};
use crate::{DataType, EnumVariants, TypeId};
use hashbrown::HashMap as GraphMap;

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

    fn codec(&self) -> Option<&dyn CustomValueCodec> {
        match &self.kind {
            TypeEntryKind::Custom { codec, .. } => codec.as_deref(),
            TypeEntryKind::Enum { .. } => None,
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
        assert!(
            !self.funcs.contains_key(&func.id),
            "duplicate function registration"
        );
        self.funcs.insert(func.id, func);
    }

    pub fn remove(&mut self, id: &FuncId) -> Option<Func> {
        self.funcs.remove(id)
    }

    pub fn graph_by_id(&self, id: &GraphId) -> Option<&Graph> {
        assert!(!id.is_nil());
        self.graphs.get(id)
    }

    /// Registers a shared graph.
    pub fn insert_graph(&mut self, id: GraphId, graph: Graph) {
        assert!(!id.is_nil());
        assert!(
            !self.graphs.contains_key(&id),
            "duplicate graph registration"
        );
        self.graphs.insert(id, graph);
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
        self.types.insert(type_id, entry);
    }

    /// The variant names of a registered `Enum` type — for the editor's enum
    /// picker and the const type-check. `None` if `type_id` is unregistered or
    /// names a non-enum type.
    pub fn enum_variants(&self, type_id: &TypeId) -> Option<&[String]> {
        assert!(!type_id.is_nil());
        self.types.get(type_id)?.variants()
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

    /// The disk codec registered for `type_id`, if any. Used by the output
    /// cache's serialize/deserialize.
    pub(crate) fn codec(&self, type_id: &TypeId) -> Option<&dyn CustomValueCodec> {
        self.types.get(type_id)?.codec()
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
    use crate::testing::{self, TestFuncHooks, test_func_lib};
    use crate::{
        CodecError, CustomValue, CustomValueCodec, DataType, DynamicValue, StaticValue, TypeId,
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

    #[test]
    fn registration_rejects_duplicate_ids_without_replacing_entries() {
        let func_id = FuncId::unique();
        let mut library = Library::default();
        library.add(testing::with_stub_lambda(Func::new(func_id, "Before")));
        let duplicate_func = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            library.add(testing::with_stub_lambda(Func::new(func_id, "After")));
        }));
        assert!(duplicate_func.is_err());
        assert_eq!(library.by_id(&func_id).unwrap().name, "Before");

        let graph_id = GraphId::unique();
        library.insert_graph(graph_id, Graph::new("Before"));
        let duplicate_graph = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            library.insert_graph(graph_id, Graph::new("After"));
        }));
        assert!(duplicate_graph.is_err());
        assert_eq!(library.graph_by_id(&graph_id).unwrap().name, "Before");

        let type_id = TypeId::unique();
        library.register_type(type_id, TypeEntry::custom("Before"));
        let duplicate_type = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            library.register_type(type_id, TypeEntry::custom("After"));
        }));
        assert!(duplicate_type.is_err());
        assert_eq!(library.types[&type_id].display_name(), "Before");
    }

    #[test]
    fn add_rejects_invalid_function_declarations() {
        for func in [
            Func::new(FuncId::nil(), "nil"),
            Func::new(FuncId::unique(), "wildcard")
                .input(FuncInput::required("value", DataType::Any))
                .wildcard_output("value", 1),
            Func::new(FuncId::unique(), "missing"),
        ] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Library::default().add(func);
            }));
            assert!(result.is_err(), "invalid declaration was registered");
        }
    }

    #[test]
    fn type_entry_kinds_expose_only_valid_codec_attachments() {
        let custom = TypeEntry::custom_with_codec("Custom", Arc::new(StubCodec));
        assert_eq!(custom.display_name(), "Custom");
        assert!(custom.variants().is_none());
        assert!(custom.codec().is_some());

        let variants = vec!["A".to_string()];
        let enum_entry = TypeEntry::enum_with_variants("Enum", variants.clone());
        assert_eq!(enum_entry.display_name(), "Enum");
        assert_eq!(enum_entry.variants(), Some(variants.as_slice()));
        assert!(enum_entry.codec().is_none());

        let mut library = Library::default();
        let custom_id = TypeId::unique();
        library.register_type(custom_id, custom);
        let enum_id = TypeId::unique();
        library.register_type(enum_id, enum_entry);
        assert!(library.codec(&custom_id).is_some());
        assert!(library.codec(&enum_id).is_none());
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
