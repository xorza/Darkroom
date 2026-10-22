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
//! than what every user of a shared fixture declares — including inside
//! [`sample`](TestGraph::sample), where a test can retarget one body without
//! reaching every other node built from the same shape.

use std::sync::Arc;

use hashbrown::HashMap;

use crate::async_lambda;
use crate::data::const_value::ConstValue;
use crate::data::data_type::DataType;
use crate::data::dynamic_value::DynamicValue;
use crate::graph::Graph;
use crate::graph::binding::Binding;
use crate::graph::detached::DetachedNode;
use crate::graph::func::Func;
use crate::graph::func::event::EventLambda;
use crate::graph::func::func_builder::FuncBuilder;
use crate::graph::func::func_input::FuncInput;
use crate::graph::func::func_output::FuncOutput;
use crate::graph::func::lambda::internals as lambda;
use crate::graph::func::lambda::{FuncLambda, Invocation};
use crate::graph::identity::{EventPort, FuncId, InputPort, NodeId};
use crate::graph::node::special::SpecialNode;
use crate::graph::node::{CacheMode, Node, NodeKind};
use crate::internals::calls::Calls;
use crate::library::Library;

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

    /// A fixture over a library built elsewhere, holding no nodes yet.
    ///
    /// For the production libraries — `system_library` and friends — whose
    /// funcs a test cannot state with [`NodeSpec`] because their bodies *are*
    /// the subject.
    pub fn over(library: Library) -> Self {
        Self {
            library,
            ..Default::default()
        }
    }

    /// Declare a func and instantiate one node of it under `name`.
    ///
    /// The node is seeded with its declaration's default const bindings, the
    /// way `Graph::add_func_node` does for an editor.
    pub fn add(&mut self, name: &str, spec: impl FnOnce(NodeSpec) -> NodeSpec) -> NodeId {
        let func = spec(NodeSpec::new(name, FuncId::from_u128(self.mint())))
            .func
            .build();
        self.library.add(func.clone());
        let node_id = self.place(&func);
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
        let node_id = self.place(&func);
        self.bind_name(name, node_id);
        node_id
    }

    /// Instantiate one node of the declaration `func_name`, which the library
    /// already holds, under that same name.
    ///
    /// The counterpart to [`add`](Self::add) for a func the fixture did not
    /// write — see [`over`](Self::over).
    pub fn add_declared(&mut self, func_name: &str) -> NodeId {
        let func = self
            .library
            .by_name(func_name)
            .unwrap_or_else(|| panic!("the library declares no func named {func_name:?}"))
            .clone();
        let node_id = self.place(&func);
        self.bind_name(func_name, node_id);
        node_id
    }

    /// Place a built-in [`SpecialNode`], whose declaration is hardcoded rather
    /// than registered.
    pub fn add_special(&mut self, name: &str, special: SpecialNode) -> NodeId {
        let mut node = Node::from(special);
        name.clone_into(&mut node.name);
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

    /// The stock five-node fixture — the graph most tests about *the engine*
    /// rather than about a topology are written over.
    ///
    /// `get_a`/`get_b` → `sum`, then `sum` and `get_b` → `mult`, then `mult` →
    /// the `Print` sink. Everything retains in RAM, so a second run exercises
    /// cross-run reuse; only `Print` is impure, so it is the one node a second
    /// run re-executes.
    pub fn sample() -> Self {
        Self::sample_values(1, 11)
    }

    /// [`sample`](Self::sample) with the two sources' values named.
    pub fn sample_values(a: i64, b: i64) -> Self {
        // The sources declare `Int` and emit `Float`: an `Int`-declared
        // consumer reading them goes through the scalar coercion class, which
        // is a path this fixture is meant to cover.
        let source = |value: i64| {
            move |n: NodeSpec| {
                n.pure()
                    .cache(CacheMode::Ram)
                    .output(DataType::Int)
                    .compute(move |_| ConstValue::Float(value as f64))
            }
        };

        let mut g = Self::new();
        g.add("get_a", source(a));
        g.add("get_b", source(b));
        g.add("sum", |n| n.sum().cache(CacheMode::Ram));
        g.add("mult", |n| n.mult().cache(CacheMode::Ram));
        g.add("Print", |n| n.records().cache(CacheMode::Ram));
        g.wire("get_a", "sum");
        g.wire("get_b", ("sum", 1));
        g.wire("sum", "mult");
        g.wire("get_b", ("mult", 1));
        g.wire("mult", "Print");
        g.graph
            .validate()
            .expect("the fixture graph is well formed");
        g
    }

    /// `source` → `sink`, plus an unwired `loose` — the three positions a node
    /// can occupy relative to a consumer edge, and the smallest graph in which
    /// every question a compile answers about a node has a distinct answer.
    pub fn source_sink_loose() -> Self {
        let mut g = Self::new();
        g.add("source", |n| n.pure().output(DataType::Int));
        g.add("sink", NodeSpec::records);
        g.add("loose", |n| n.pure().output(DataType::Int));
        g.wire("source", "sink");
        g
    }

    pub fn id(&self, name: &str) -> NodeId {
        *self
            .ids
            .get(name)
            .unwrap_or_else(|| panic!("no node named {name:?} in this fixture"))
    }

    /// Bind `consumer`'s input to `producer`'s output: a bare name is the node's port 0.
    pub fn wire<'a>(
        &mut self,
        producer: impl Into<NamedPort<'a>>,
        consumer: impl Into<NamedPort<'a>>,
    ) {
        let (producer, consumer) = (producer.into(), consumer.into());
        let binding = Binding::wire(self.id(producer.node), producer.idx);
        self.set(consumer.node, consumer.idx, binding);
    }

    /// Put a literal on `consumer`'s input `input`.
    pub fn constant(&mut self, consumer: &str, input: usize, value: impl Into<ConstValue>) {
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
        let event = EventPort::new(self.id(emitter), event_idx);
        self.graph.subscribe(event, self.id(subscriber));
    }

    pub fn unsubscribe(&mut self, emitter: &str, event_idx: usize, subscriber: &str) {
        let event = EventPort::new(self.id(emitter), event_idx);
        self.graph.unsubscribe(event, self.id(subscriber));
    }

    pub fn disable(&mut self, name: &str) {
        self.node_mut(name).disabled = true;
    }

    pub fn enable(&mut self, name: &str) {
        self.node_mut(name).disabled = false;
    }

    pub fn cache(&mut self, name: &str, mode: CacheMode) {
        self.node_mut(name).cache = mode;
    }

    /// Put every node on one cache mode — how a test states "nothing is
    /// retained here" without listing the graph.
    pub fn cache_all(&mut self, mode: CacheMode) {
        for &node_id in self.ids.values() {
            self.graph
                .find_mut(node_id)
                .expect("a named node is in the graph")
                .cache = mode;
        }
    }

    /// Edit the declaration `name` instantiates, in place.
    ///
    /// The library is a registry keyed by id, so this is remove-edit-re-add —
    /// which also re-runs [`FuncBuilder::build`]'s checks, so an edit that
    /// breaks the declaration fails here rather than at the next compile. Under
    /// [`instance`](Self::instance) the edit reaches every node of that func,
    /// which is what sharing a declaration means.
    pub fn edit_func(&mut self, name: &str, edit: impl FnOnce(&mut FuncBuilder)) {
        let func_id = self.func_id(name);
        let mut builder = self
            .library
            .remove(func_id)
            .expect("the named node's func is registered")
            .into_builder();
        edit(&mut builder);
        self.library.add(builder.build());
    }

    /// [`edit_func`](Self::edit_func) for an edit that moves ports, with the
    /// graph adopting the new ones: every node of the func records the new
    /// signature, as an author who accepted the change would leave it. Without
    /// that, the guard refuses the nodes the edit left behind.
    pub fn evolve_func(&mut self, name: &str, edit: impl FnOnce(&mut FuncBuilder)) {
        self.edit_func(name, edit);
        let func_id = self.func_id(name);
        let cur = self.library.by_id(func_id).unwrap().func_ref();
        let nodes: Vec<NodeId> = self
            .graph
            .iter()
            .filter(|node| node.func_id() == Some(func_id))
            .map(|node| node.node_id)
            .collect();
        for node in nodes {
            self.graph.find_mut(node).unwrap().kind = NodeKind::Func(cur);
        }
    }

    /// Replace what `name` declares with a body that always fails — the
    /// edit-time form of [`NodeSpec::fails`], for a node the fixture did not
    /// spec itself.
    pub fn fails(&mut self, name: &str, message: &'static str) {
        self.edit_func(name, |func| func.lambda = failing_lambda(message));
    }

    /// Replace what `name` declares with a body that panics if a run reaches
    /// it.
    ///
    /// How a fixture states "this must be pruned" as something the run has to
    /// honour, rather than as an assertion made after the fact about what the
    /// outcome listed.
    pub fn never(&mut self, name: &str) {
        let reason = format!("{name} must not run in this fixture");
        self.edit_func(name, move |func| {
            func.lambda = async_lambda!(
                move |_| { reason = reason.clone() } => { panic!("{reason}") }
            );
        });
    }

    /// [`never`](Self::never) on every node — "nothing here runs at all".
    pub fn never_all(&mut self) {
        #[expect(
            clippy::needless_collect,
            reason = "`never` borrows the whole fixture, so the names cannot stay borrowed"
        )]
        for name in self.ids.keys().cloned().collect::<Vec<_>>() {
            self.never(&name);
        }
    }

    fn func_id(&self, name: &str) -> FuncId {
        match self.node(name).func_id() {
            Some(func_id) => func_id,
            None => {
                panic!("{name:?} is a special node, whose declaration is hardcoded")
            }
        }
    }

    fn node(&self, name: &str) -> &Node {
        self.graph
            .find(self.id(name))
            .expect("a named node is in the graph")
    }

    fn node_mut(&mut self, name: &str) -> &mut Node {
        let node_id = self.id(name);
        self.graph
            .find_mut(node_id)
            .expect("a named node is in the graph")
    }

    /// Place one node of `func` under the next ascending id, seeding its
    /// declared const defaults the way `Graph::add_func_node` does for an
    /// editor.
    fn place(&mut self, func: &Func) -> NodeId {
        let node_id = NodeId::from_u128(self.mint());
        self.graph.insert(node_id, Node::from(func));
        self.graph.bindings.extend(
            func.default_bindings(node_id)
                .map(|entry| (entry.port, entry.binding)),
        );
        node_id
    }

    const fn mint(&mut self) -> u128 {
        self.next_id += 1;
        self.next_id
    }

    fn bind_name(&mut self, name: &str, node_id: NodeId) {
        let prev = self.ids.insert(name.to_owned(), node_id);
        assert!(prev.is_none(), "a fixture reused the name {name:?}");
    }
}

/// The body both `fails` methods install, so the two cannot drift.
fn failing_lambda(message: &'static str) -> FuncLambda {
    async_lambda!(move |_| { Err(lambda::failure(message)) })
}

/// The type a scalar or string literal names. A path is a bare string without
/// the mode and extensions of a `DataType::FsPath`, an enum variant a name
/// without its type's id, and `Null` names nothing.
fn literal_type(value: &ConstValue) -> DataType {
    match value {
        ConstValue::Float(_) => DataType::Float,
        ConstValue::Int(_) => DataType::Int,
        ConstValue::Bool(_) => DataType::Bool,
        ConstValue::String(_) => DataType::String,
        ConstValue::Null | ConstValue::FsPath(_) | ConstValue::FsPaths(_) | ConstValue::Enum(_) => {
            panic!("{value:?} names no type an output could declare")
        }
    }
}

/// A port of a fixture's node, by the node's name: a bare `&str` is its port 0, a
/// `(name, idx)` pair any other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamedPort<'a> {
    pub node: &'a str,
    pub idx: usize,
}

impl<'a> From<&'a str> for NamedPort<'a> {
    fn from(node: &'a str) -> Self {
        Self { node, idx: 0 }
    }
}

impl<'a> From<&'a String> for NamedPort<'a> {
    fn from(node: &'a String) -> Self {
        Self::from(node.as_str())
    }
}

impl<'a> From<(&'a str, usize)> for NamedPort<'a> {
    fn from((node, idx): (&'a str, usize)) -> Self {
        Self { node, idx }
    }
}

/// One node's declaration under construction — [`Func`]'s builders, plus the
/// bodies a fixture keeps writing by hand.
///
/// Ports are named `in0`, `out0`, … by position: a test that cares about a
/// port's *name* is testing the editor's rendering, which builds its own funcs.
#[derive(Debug)]
pub struct NodeSpec {
    func: FuncBuilder,
}

impl NodeSpec {
    fn new(name: &str, func_id: FuncId) -> Self {
        // A stub body by default, because `Library::add` rejects a func with
        // no implementation and most fixtures never invoke one.
        Self {
            func: Func::builder(func_id, name, FuncLambda::stub()),
        }
    }

    #[must_use]
    pub fn pure(mut self) -> Self {
        self.func = self.func.pure();
        self
    }

    #[must_use]
    pub fn sink(mut self) -> Self {
        self.func = self.func.sink();
        self
    }

    /// The [`CacheMode`] nodes of this func start at.
    #[must_use]
    pub fn cache(mut self, mode: CacheMode) -> Self {
        self.func = self.func.default_cache_mode(mode);
        self
    }

    #[must_use]
    pub fn input(mut self, data_type: DataType) -> Self {
        let name = format!("in{}", self.func.inputs.len());
        self.func = self.func.input(FuncInput::required(name, data_type));
        self
    }

    /// A required input that may only hold a literal — wiring an upstream
    /// output into it is what graph validation rejects.
    #[must_use]
    pub fn const_only(mut self) -> Self {
        let last = self
            .func
            .inputs
            .last_mut()
            .expect("`const_only` restricts the input declared before it");
        last.const_only = true;
        self
    }

    #[must_use]
    pub fn optional(mut self, data_type: DataType) -> Self {
        let name = format!("in{}", self.func.inputs.len());
        self.func = self.func.input(FuncInput::optional(name, data_type));
        self
    }

    /// An optional input carrying a declared default, so a fresh node of this
    /// func starts with that literal already bound.
    #[must_use]
    pub fn defaulted(mut self, data_type: DataType, value: impl Into<ConstValue>) -> Self {
        let name = format!("in{}", self.func.inputs.len());
        self.func = self
            .func
            .input(FuncInput::optional(name, data_type).default(value.into()));
        self
    }

    #[must_use]
    pub fn output(mut self, data_type: DataType) -> Self {
        let name = format!("out{}", self.func.outputs.len());
        self.func = self.func.output(FuncOutput::new(name, data_type));
        self
    }

    #[must_use]
    pub fn event(mut self, name: &str, lambda: EventLambda) -> Self {
        self.func = self.func.event(name, lambda);
        self
    }

    #[must_use]
    pub fn lambda(mut self, lambda: FuncLambda) -> Self {
        self.func.lambda = lambda;
        self
    }

    /// A body writing `body(inputs)` to output 0.
    ///
    /// Declare the ports first — this supplies only the implementation, and
    /// panics at run time if the node declares no output to write.
    /// [`observes`](Self::observes) is the form for a node with none.
    #[must_use]
    pub fn compute(
        self,
        body: impl Fn(&[DynamicValue]) -> ConstValue + Send + Sync + 'static,
    ) -> Self {
        let body = Arc::new(body);
        self.lambda(async_lambda!(
            move |Invocation { inputs, outputs, .. }| { body = Arc::clone(&body) } => {
                outputs[0] = DynamicValue::Static(body(inputs));
                Ok(())
            }
        ))
    }

    /// A body that only *looks* at its inputs, writing nothing.
    ///
    /// For a sink whose effect is recorded outside the graph. Unlike
    /// [`compute`](Self::compute) it declares and writes no output, so it fits
    /// a node that has none.
    #[must_use]
    pub fn observes(self, body: impl Fn(&[DynamicValue]) + Send + Sync + 'static) -> Self {
        let body = Arc::new(body);
        self.lambda(async_lambda!(
            move |Invocation { inputs, .. }| { body = Arc::clone(&body) } => {
                body(inputs);
                Ok(())
            }
        ))
    }

    /// A pure source of one scalar or string constant: declares the output
    /// too, typed from the literal.
    #[must_use]
    pub fn returns(self, value: impl Into<ConstValue>) -> Self {
        let value = value.into();
        let data_type = literal_type(&value);
        self.pure()
            .output(data_type)
            .compute(move |_| value.clone())
    }

    /// [`returns`](Self::returns), counting each call — the source every "did
    /// the upstream recompute" fixture is built on, since `calls` says both
    /// whether the node ran and how often.
    #[must_use]
    pub fn counted(self, value: impl Into<ConstValue>, calls: &Calls) -> Self {
        let value = value.into();
        self.returns(value.clone()).compute(calls.returning(value))
    }

    /// A pure two-input arithmetic node: `in0 op in1`, both `Int`, one `Int`
    /// output.
    ///
    /// The second input is **optional and defaults to `identity`**, so a
    /// fixture may leave it unbound or const-bind it to move the node's digest
    /// without touching its producers.
    #[must_use]
    pub fn arith(self, identity: i64, op: fn(i64, i64) -> i64) -> Self {
        self.pure()
            .input(DataType::Int)
            .optional(DataType::Int)
            .output(DataType::Int)
            .compute(move |inputs| {
                let a = inputs[0].as_i64().unwrap();
                let b = inputs[1].as_i64().unwrap_or(identity);
                op(a, b).into()
            })
    }

    /// [`arith`](Self::arith) adding its inputs, identity `0`.
    #[must_use]
    pub fn sum(self) -> Self {
        self.arith(0, |a, b| a + b)
    }

    /// [`arith`](Self::arith) multiplying its inputs, identity `1`.
    #[must_use]
    pub fn mult(self) -> Self {
        self.arith(1, |a, b| a * b)
    }

    /// A body that always fails, for the error-propagation paths — the
    /// consumer cascade is what such a node exists to provoke.
    ///
    /// [`TestGraph::fails`] is the same body installed on a declaration the
    /// fixture did not spec itself.
    #[must_use]
    pub fn fails(self, message: &'static str) -> Self {
        self.lambda(failing_lambda(message))
    }

    /// A sink that logs whatever reaches it, readable back off the run's log
    /// lines — how a fixture observes that a node ran *and* what it saw,
    /// without a shared hook.
    ///
    /// **Declares its own port**: one required `Any` input, which is the one it
    /// logs. Adding another input before this leaves that port unfed, which
    /// blocks the node rather than logging anything.
    #[must_use]
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
pub(crate) mod compiled;

#[cfg(test)]
mod internals {
    use std::sync::Arc;

    use crate::exec::compile::Compiler;
    use crate::exec::compile::compiled_graph::CompiledGraph;
    use crate::graph::identity::{EventPort, NodeId};
    use crate::internals::test_graph::TestGraph;
    use crate::internals::test_graph::compiled::Compiled;

    impl TestGraph {
        /// Every node the fixture named, under that name — which, for an
        /// [`instance`](Self::instance), is not the node's own (its func's).
        pub(crate) fn names(&self) -> impl Iterator<Item = (NodeId, &str)> {
            self.ids
                .iter()
                .map(|(name, node_id)| (*node_id, name.as_str()))
        }

        /// `name`'s event `event_idx`.
        pub(crate) fn event(&self, name: &str, event_idx: usize) -> EventPort {
            EventPort::new(self.id(name), event_idx)
        }

        /// This fixture lowered, as an install or a message carries it.
        pub(crate) fn program(&self) -> Arc<CompiledGraph> {
            Arc::new(self.compile().program)
        }

        /// Lower this fixture, keeping the names.
        pub(crate) fn compile(&self) -> Compiled {
            Compiled::new(
                Compiler::default().compile(&self.graph, &self.library),
                self.ids.clone(),
            )
        }
    }
}

#[cfg(test)]
mod tests;
