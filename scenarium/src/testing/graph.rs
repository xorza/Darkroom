//! [`TestGraph`]: an authoring graph and the library it resolves against,
//! built together and addressed by **name**.
//!
//! A graph is five concepts to state by hand — a func, an id for it, a library
//! to hold it, a node instantiated from it, and a port-indexed binding — and a
//! test that only wants "a feeds b" has to say all five. Here a node *is* its
//! declaration: [`add`](TestGraph::add) mints both, ids stay internal, and
//! every later call names the node the way the test already thinks of it.
//!
//! **One func per node.** Two nodes never share a declaration unless a test
//! asks with [`instance`](TestGraph::instance), so
//! [`edit_func`](TestGraph::edit_func) changes what one node declares rather
//! than what every user of a shared fixture declares. That is the difference
//! from the older `test_func_lib`, whose five funcs are edited in place by
//! whichever test needs a different shape.

use std::sync::Arc;

use hashbrown::HashMap;

use crate::async_lambda;
use crate::graph::detached::DetachedNode;
use crate::graph::func::error::InvokeError;
use crate::graph::func::event::EventLambda;
use crate::graph::func::lambda::{FuncLambda, Invocation};
use crate::graph::func::{Func, FuncInput, FuncOutput};
use crate::graph::identity::{FuncId, InputPort, NodeId};
use crate::graph::node::special::SpecialNode;
use crate::graph::node::{CacheMode, Node, NodeKind};
use crate::graph::{Binding, Graph};
use crate::library::Library;
use crate::testing::{TestFuncHooks, test_func_lib, test_graph};
use crate::{DataType, DynamicValue, StaticValue};

/// A graph, the library it resolves against, and the names its nodes answer to.
///
/// Both halves are public: a test that needs the real API — `Compiler`,
/// `Graph::produces_cycle`, a raw `Binding` — reaches straight for them, and
/// the methods here are only the shorthand for what a fixture does over and
/// over.
#[derive(Debug, Default)]
pub struct TestGraph {
    pub graph: Graph,
    pub library: Library,
    ids: HashMap<String, NodeId>,
    /// The next identity to mint. Ids **ascend with declaration order**, which
    /// a compile then places in — so a fixture's nodes land in the order the
    /// test wrote them, and an assertion about schedule order is reproducible.
    ///
    /// Minting `NodeId::unique()` here instead would make the dense index space
    /// depend on how four random uuids happened to sort, and any test naming a
    /// run order would pass or fail per process.
    next_id: u128,
}

impl TestGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare a func and instantiate one node of it under `name`.
    ///
    /// The node is seeded with its declaration's default const bindings, the
    /// way `Graph::add_func_node` does for an editor.
    pub fn add(&mut self, name: &str, spec: impl FnOnce(NodeSpec) -> NodeSpec) -> NodeId {
        let func = spec(NodeSpec::new(name, FuncId::from_u128(self.mint()))).func;
        self.library.add(func.clone());
        let node_id = self.place(func_node(&func), &func);
        self.bind_name(name, node_id);
        node_id
    }

    /// A second node of the func `of` already declares — the one way two nodes
    /// share a declaration, so that sharing is always something a test asked
    /// for.
    pub fn instance(&mut self, name: &str, of: &str) -> NodeId {
        let func = self
            .library
            .by_id(self.func_id(of))
            .expect("the named node's func is registered")
            .clone();
        let node_id = self.place(func_node(&func), &func);
        self.bind_name(name, node_id);
        node_id
    }

    /// Place a built-in [`SpecialNode`], whose declaration is hardcoded rather
    /// than registered.
    pub fn add_special(&mut self, name: &str, special: SpecialNode) -> NodeId {
        let mut node = Node::new(NodeKind::Special(special));
        node.name = name.to_owned();
        let node_id = NodeId::from_u128(self.mint());
        self.graph.insert(node_id, node);
        self.bind_name(name, node_id);
        node_id
    }

    /// Remove a node and every edge that touched it, freeing its name.
    ///
    /// The declaration stays registered: a library outliving the nodes that
    /// used it is the ordinary case, and re-adding under the same name would
    /// otherwise collide on the func id.
    pub fn remove(&mut self, name: &str) -> DetachedNode {
        let node_id = self.id(name);
        self.ids.remove(name);
        self.graph.detach_node(node_id)
    }

    /// Adopt a graph and library built elsewhere, taking each node's name from
    /// the node itself. Nodes sharing a name resolve to an arbitrary one of
    /// them — the same rule the old `Graph::find_by_name` followed.
    pub fn adopt(graph: Graph, library: Library) -> Self {
        let ids = graph
            .iter()
            .map(|node| (node.name.clone(), node.id))
            .collect();
        Self {
            graph,
            library,
            ids,
            next_id: 0,
        }
    }

    /// The legacy five-node fixture — `get_a`/`get_b` → `sum` → `mult` → `Print`
    /// — with hooks that answer instead of panicking.
    ///
    /// A bridge, not an example: it delegates to `test_graph`/`test_func_lib`
    /// so node ids (and therefore the dense order a compile assigns) stay
    /// exactly what they were, and tests can migrate one at a time.
    pub fn sample() -> Self {
        Self::sample_with(TestFuncHooks {
            get_a: Arc::new(|| Ok(1)),
            get_b: Arc::new(|| 11),
            print: Arc::new(|_| {}),
        })
    }

    /// [`sample`](Self::sample) with the legacy fixture's hooks named — for a
    /// test whose subject is what one of those bodies *does* (fails, counts its
    /// calls, records what it was handed).
    pub fn sample_with(hooks: TestFuncHooks) -> Self {
        Self::adopt(test_graph(), test_func_lib(hooks))
    }

    pub fn id(&self, name: &str) -> NodeId {
        *self
            .ids
            .get(name)
            .unwrap_or_else(|| panic!("no node named {name:?} in this fixture"))
    }

    /// Bind `consumer`'s input `input` to `producer`'s output `out`.
    pub fn wire(&mut self, producer: &str, out: usize, consumer: &str, input: usize) {
        let binding = Binding::bind(self.id(producer), out);
        self.set(consumer, input, binding);
    }

    /// Put a literal on `consumer`'s input `input`.
    pub fn constant(&mut self, consumer: &str, input: usize, value: impl Into<StaticValue>) {
        self.set(consumer, input, Binding::Const(value.into()));
    }

    /// Leave `consumer`'s input `input` with nothing on it.
    pub fn unbind(&mut self, consumer: &str, input: usize) {
        self.set(consumer, input, None);
    }

    fn set(&mut self, consumer: &str, input: usize, binding: impl Into<Option<Binding>>) {
        let port = InputPort::new(self.id(consumer), input);
        self.graph.set_input_binding(port, binding);
    }

    pub fn subscribe(&mut self, emitter: &str, event_idx: usize, subscriber: &str) {
        let (emitter, subscriber) = (self.id(emitter), self.id(subscriber));
        self.graph.subscribe(emitter, event_idx, subscriber);
    }

    pub fn unsubscribe(&mut self, emitter: &str, event_idx: usize, subscriber: &str) {
        let (emitter, subscriber) = (self.id(emitter), self.id(subscriber));
        self.graph.unsubscribe(emitter, event_idx, subscriber);
    }

    pub fn disable(&mut self, name: &str) {
        self.node_mut(name).disabled = true;
    }

    pub fn cache(&mut self, name: &str, mode: CacheMode) {
        self.node_mut(name).cache = mode;
    }

    /// Put every node on one cache mode — how a test states "nothing is
    /// retained here" without listing the graph.
    pub fn cache_all(&mut self, mode: CacheMode) {
        for node_id in self.ids.values().copied().collect::<Vec<_>>() {
            self.graph
                .find_mut(node_id)
                .expect("a named node is in the graph")
                .cache = mode;
        }
    }

    /// Edit the declaration `name` instantiates, in place.
    ///
    /// The library is a registry keyed by id, so this is remove-edit-re-add —
    /// which also re-runs `Func::validate`, so an edit that breaks the
    /// declaration fails here rather than at the next compile. Under
    /// [`instance`](Self::instance) the edit reaches every node of that func,
    /// which is what sharing a declaration means.
    pub fn edit_func(&mut self, name: &str, edit: impl FnOnce(&mut Func)) {
        let func_id = self.func_id(name);
        let mut func = self
            .library
            .remove(func_id)
            .expect("the named node's func is registered");
        edit(&mut func);
        self.library.add(func);
    }

    fn func_id(&self, name: &str) -> FuncId {
        match self.node(name).kind {
            NodeKind::Func(func_id) => func_id,
            NodeKind::Special(_) => {
                panic!("{name:?} is a special node, whose declaration is hardcoded")
            }
        }
    }

    fn node(&self, name: &str) -> &crate::graph::node::Node {
        self.graph
            .find(self.id(name))
            .expect("a named node is in the graph")
    }

    fn node_mut(&mut self, name: &str) -> &mut crate::graph::node::Node {
        let node_id = self.id(name);
        self.graph
            .find_mut(node_id)
            .expect("a named node is in the graph")
    }

    /// Place `node` under the next ascending id, seeding its declared const
    /// defaults the way `Graph::add_func_node` does for an editor.
    fn place(&mut self, node: Node, func: &Func) -> NodeId {
        let node_id = NodeId::from_u128(self.mint());
        self.graph.insert(node_id, node);
        self.graph.bindings.extend(func.default_bindings(node_id));
        node_id
    }

    fn mint(&mut self) -> u128 {
        self.next_id += 1;
        self.next_id
    }

    fn bind_name(&mut self, name: &str, node_id: NodeId) {
        let previous = self.ids.insert(name.to_owned(), node_id);
        assert!(previous.is_none(), "a fixture reused the name {name:?}");
    }
}

/// A bare node of `func`, before its id is chosen.
fn func_node(func: &Func) -> Node {
    Node::from(func)
}

/// One node's declaration under construction — [`Func`]'s builders, plus the
/// bodies a fixture keeps writing by hand.
///
/// Ports are named `in0`, `out0`, … by position: a test that cares about a
/// port's *name* is testing the editor's rendering, which builds its own funcs.
#[derive(Debug)]
pub struct NodeSpec {
    func: Func,
}

impl NodeSpec {
    fn new(name: &str, func_id: FuncId) -> Self {
        // A stub body by default, because `Library::add` rejects a func with
        // no implementation and most fixtures never invoke one.
        Self {
            func: Func::new(func_id, name).lambda(async_lambda!(|_| { Ok(()) })),
        }
    }

    pub fn pure(mut self) -> Self {
        self.func = self.func.pure();
        self
    }

    pub fn sink(mut self) -> Self {
        self.func = self.func.sink();
        self
    }

    pub fn uncacheable(mut self) -> Self {
        self.func = self.func.uncacheable();
        self
    }

    /// The [`CacheMode`] nodes of this func start at.
    pub fn cache(mut self, mode: CacheMode) -> Self {
        self.func = self.func.default_cache_mode(mode);
        self
    }

    pub fn input(mut self, data_type: DataType) -> Self {
        let name = format!("in{}", self.func.inputs.len());
        self.func = self.func.input(FuncInput::required(name, data_type));
        self
    }

    pub fn optional(mut self, data_type: DataType) -> Self {
        let name = format!("in{}", self.func.inputs.len());
        self.func = self.func.input(FuncInput::optional(name, data_type));
        self
    }

    /// An optional input carrying a declared default, so a fresh node of this
    /// func starts with that literal already bound.
    pub fn defaulted(mut self, data_type: DataType, value: impl Into<StaticValue>) -> Self {
        let name = format!("in{}", self.func.inputs.len());
        self.func = self
            .func
            .input(FuncInput::optional(name, data_type).default(value.into()));
        self
    }

    pub fn output(mut self, data_type: DataType) -> Self {
        let name = format!("out{}", self.func.outputs.len());
        self.func = self.func.output(FuncOutput::new(name, data_type));
        self
    }

    /// An output mirroring input `mirrors` — a passthrough / reroute port.
    pub fn wildcard(mut self, mirrors: usize) -> Self {
        let name = format!("out{}", self.func.outputs.len());
        self.func = self.func.wildcard_output(name, mirrors);
        self
    }

    pub fn event(mut self, name: &str, lambda: EventLambda) -> Self {
        self.func = self.func.event(name, lambda);
        self
    }

    pub fn lambda(mut self, lambda: FuncLambda) -> Self {
        self.func = self.func.lambda(lambda);
        self
    }

    /// A body writing `body(inputs)` to output 0.
    ///
    /// Declare the ports first — this supplies only the implementation, and
    /// panics at run time if the node declares no output to write.
    pub fn compute(
        self,
        body: impl Fn(&[DynamicValue]) -> StaticValue + Send + Sync + 'static,
    ) -> Self {
        let body = Arc::new(body);
        self.lambda(async_lambda!(
            move |Invocation { inputs, outputs, .. }| { body = Arc::clone(&body) } => {
                outputs[0] = DynamicValue::Static(body(inputs));
                Ok(())
            }
        ))
    }

    /// A pure source of one constant: declares the output too, typed from the
    /// literal (`Any` for a literal that names no type of its own — a path, an
    /// enum variant, `Null`).
    pub fn returns(self, value: impl Into<StaticValue>) -> Self {
        let value = value.into();
        let data_type = DataType::Any.or_const_type(&value);
        self.pure()
            .output(data_type)
            .compute(move |_| value.clone())
    }

    /// A body that always fails, for the error-propagation paths — the
    /// consumer cascade is what such a node exists to provoke.
    pub fn fails(self, message: &'static str) -> Self {
        self.lambda(async_lambda!(move |_| {
            Err(InvokeError::external(std::io::Error::other(message)))
        }))
    }

    /// A sink that logs whatever reaches it, readable back off the run's
    /// [`logs`](crate::execution::report::ExecutionOutcome) — how a fixture
    /// observes that a node ran *and* what it saw, without a shared hook.
    ///
    /// **Declares its own port**: one required `Any` input, which is the one it
    /// logs. Adding another input before this leaves that port unfed, which
    /// blocks the node rather than logging anything.
    pub fn records(self) -> Self {
        self.sink().input(DataType::Any).lambda(async_lambda!(
            move |Invocation { ctx, inputs, .. }| {
                ctx.info(inputs[0].to_value_string());
                Ok(())
            }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::compile::Compiler;
    use crate::graph::func::FuncBehavior;

    /// The builder states a whole graph — declarations, wiring and bodies — and
    /// what it produces compiles.
    #[test]
    fn a_named_graph_compiles_to_one_node_per_name() {
        let mut g = TestGraph::new();
        g.add("src", |n| n.returns(7i64));
        g.add("double", |n| {
            n.pure()
                .input(DataType::Int)
                .output(DataType::Int)
                .compute(|inputs| (inputs[0].as_i64().unwrap() * 2).into())
        });
        g.add("print", |n| n.records());
        g.wire("src", 0, "double", 0);
        g.wire("double", 0, "print", 0);

        // Every name resolves to a distinct node, and the wiring is real.
        assert_eq!(g.graph.len(), 3);
        assert_eq!(
            g.graph.bindings[&InputPort::new(g.id("double"), 0)],
            Binding::bind(g.id("src"), 0)
        );
        // `returns` declared the output its literal implies.
        let compiled = Compiler::default()
            .compile(&g.graph, &g.library)
            .expect("the fixture compiles");
        assert!(compiled.contains(g.id("src")));
        assert_eq!(compiled.node_ids.len(), 3);
    }

    /// One func per node by default: editing one node's declaration leaves
    /// every other node's alone. `instance` is the only way to opt into
    /// sharing, and then the edit reaches both.
    #[test]
    fn declarations_are_per_node_unless_a_test_shares_one() {
        let mut g = TestGraph::new();
        g.add("a", |n| n.pure().input(DataType::Int).output(DataType::Int));
        g.add("b", |n| n.pure().input(DataType::Int).output(DataType::Int));
        g.instance("a2", "a");

        g.edit_func("a", |func| func.inputs[0].required = false);

        let required = |g: &TestGraph, name: &str| {
            let node = g.graph.find(g.id(name)).unwrap();
            g.graph.node_func(node, &g.library).unwrap().inputs[0].required
        };
        assert!(!required(&g, "a"), "the edited declaration went optional");
        assert!(!required(&g, "a2"), "and its other instance shares it");
        assert!(required(&g, "b"), "an unrelated node is untouched");
    }

    /// `defaulted` seeds the const binding a fresh node starts with, and
    /// `constant`/`unbind` overwrite it — the three ways a literal reaches a
    /// port.
    #[test]
    fn declared_defaults_seed_bindings_that_later_calls_replace() {
        let mut g = TestGraph::new();
        g.add("n", |n| {
            n.pure()
                .defaulted(DataType::Int, 3i64)
                .output(DataType::Int)
        });
        let port = InputPort::new(g.id("n"), 0);

        assert_eq!(
            g.graph.bindings[&port],
            Binding::Const(StaticValue::Int(3)),
            "the declaration's default is bound on instantiation"
        );

        g.constant("n", 0, 9i64);
        assert_eq!(g.graph.bindings[&port], Binding::Const(StaticValue::Int(9)));

        g.unbind("n", 0);
        assert!(!g.graph.bindings.contains_key(&port));
    }

    /// `sample()` is the legacy fixture unchanged — same nodes under the same
    /// ids, which is what lets a test migrate to the harness without its
    /// assertions about schedule order moving.
    #[test]
    fn sample_preserves_the_legacy_fixtures_identities() {
        let g = TestGraph::sample();
        let legacy = test_graph();

        assert_eq!(g.graph.len(), legacy.len());
        for node in legacy.iter() {
            assert_eq!(
                g.id(&node.name),
                node.id,
                "{} keeps the id the legacy fixture gave it",
                node.name
            );
        }
        assert_eq!(g.graph, legacy, "and the wiring is identical");
    }

    /// The spec's flags land on the declaration rather than being remembered
    /// by the builder — distinct inputs produce distinct funcs.
    #[test]
    fn spec_flags_reach_the_declaration() {
        let mut g = TestGraph::new();
        g.add("plain", |n| n.output(DataType::Int));
        g.add("flagged", |n| {
            n.pure()
                .sink()
                .uncacheable()
                .cache(CacheMode::Both)
                .output(DataType::Int)
        });

        let func = |g: &TestGraph, name: &str| {
            let node = g.graph.find(g.id(name)).unwrap();
            g.graph.node_func(node, &g.library).unwrap().clone()
        };
        let plain = func(&g, "plain");
        assert_eq!(plain.behavior, FuncBehavior::Impure);
        assert!(!plain.sink && !plain.uncacheable);
        assert_eq!(plain.default_cache_mode, CacheMode::None);

        let flagged = func(&g, "flagged");
        assert_eq!(flagged.behavior, FuncBehavior::Pure);
        assert!(flagged.sink && flagged.uncacheable);
        assert_eq!(flagged.default_cache_mode, CacheMode::Both);
    }
}
