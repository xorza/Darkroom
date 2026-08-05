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
use crate::testing::calls::Calls;
use crate::{ConstValue, DataType, DynamicValue};

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
        let func = spec(NodeSpec::new(name, FuncId::from_u128(self.mint()))).func;
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
        g.wire("get_a", 0, "sum", 0);
        g.wire("get_b", 0, "sum", 1);
        g.wire("sum", 0, "mult", 0);
        g.wire("get_b", 0, "mult", 1);
        g.wire("mult", 0, "Print", 0);
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
        g.add("sink", |n| n.records());
        g.add("loose", |n| n.pure().output(DataType::Int));
        g.wire("source", 0, "sink", 0);
        g
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
        for name in self.ids.keys().cloned().collect::<Vec<_>>() {
            self.never(&name);
        }
    }

    fn func_id(&self, name: &str) -> FuncId {
        match self.node(name).kind {
            NodeKind::Func(func_id) => func_id,
            NodeKind::Special(_) => {
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

/// The body both `fails` methods install, so the two cannot drift.
fn failing_lambda(message: &'static str) -> FuncLambda {
    async_lambda!(move |_| { Err(InvokeError::external(std::io::Error::other(message))) })
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

    /// A required input that may only hold a literal — wiring an upstream
    /// output into it is what graph validation rejects.
    pub fn const_only(mut self) -> Self {
        let last = self
            .func
            .inputs
            .last_mut()
            .expect("`const_only` restricts the input declared before it");
        last.const_only = true;
        self
    }

    pub fn optional(mut self, data_type: DataType) -> Self {
        let name = format!("in{}", self.func.inputs.len());
        self.func = self.func.input(FuncInput::optional(name, data_type));
        self
    }

    /// An optional input carrying a declared default, so a fresh node of this
    /// func starts with that literal already bound.
    pub fn defaulted(mut self, data_type: DataType, value: impl Into<ConstValue>) -> Self {
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
    /// [`observes`](Self::observes) is the form for a node with none.
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
    pub fn observes(self, body: impl Fn(&[DynamicValue]) + Send + Sync + 'static) -> Self {
        let body = Arc::new(body);
        self.lambda(async_lambda!(
            move |Invocation { inputs, .. }| { body = Arc::clone(&body) } => {
                body(inputs);
                Ok(())
            }
        ))
    }

    /// A pure source of one constant: declares the output too, typed from the
    /// literal (`Any` for a literal that names no type of its own — a path, an
    /// enum variant, `Null`).
    pub fn returns(self, value: impl Into<ConstValue>) -> Self {
        let value = value.into();
        let data_type = DataType::Any.or_const_type(&value);
        self.pure()
            .output(data_type)
            .compute(move |_| value.clone())
    }

    /// [`returns`](Self::returns), counting each call — the source every "did
    /// the upstream recompute" fixture is built on, since `calls` says both
    /// whether the node ran and how often.
    pub fn counted(self, value: impl Into<ConstValue>, calls: &Calls) -> Self {
        let value = value.into();
        let data_type = DataType::Any.or_const_type(&value);
        self.pure()
            .output(data_type)
            .compute(calls.returning(value))
    }

    /// A pure two-input arithmetic node: `in0 op in1`, both `Int`, one `Int`
    /// output.
    ///
    /// The second input is **optional and defaults to `identity`**, so a
    /// fixture may leave it unbound or const-bind it to move the node's digest
    /// without touching its producers.
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
    pub fn sum(self) -> Self {
        self.arith(0, |a, b| a + b)
    }

    /// [`arith`](Self::arith) multiplying its inputs, identity `1`.
    pub fn mult(self) -> Self {
        self.arith(1, |a, b| a * b)
    }

    /// A body that always fails, for the error-propagation paths — the
    /// consumer cascade is what such a node exists to provoke.
    ///
    /// [`TestGraph::fails`] is the same body installed on a declaration the
    /// fixture did not spec itself.
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
mod tests;
