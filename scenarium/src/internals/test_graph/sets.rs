//! What the tests of a set of graphs share: a library of a few plain funcs, and short
//! constructors for the node and compound ids a test picks by hand.

use crate::data::data_type::DataType;
use crate::graph::identity::{CompoundId, FuncId, NodeId};
use crate::graph::node::Node;
use crate::graph::ports::{InputDecl, OutputDecl};
use crate::internals;
use crate::library::Library;

/// One `Int` output, `value`.
pub(crate) const SRC: FuncId = FuncId::from_u128(1);
/// Two required `Int` inputs, `a` and `b`, to one output, `sum`.
pub(crate) const ADD: FuncId = FuncId::from_u128(2);
/// A sink of one required `Int`.
pub(crate) const SINK: FuncId = FuncId::from_u128(3);
/// An event, `tick`, and nothing else.
pub(crate) const TICKER: FuncId = FuncId::from_u128(4);
/// A sink of one required `Any`.
pub(crate) const ANY_SINK: FuncId = FuncId::from_u128(5);
/// A sink of one required, const-only `Int`.
pub(crate) const CONFIG_SINK: FuncId = FuncId::from_u128(6);

pub(crate) const fn n(id: u128) -> NodeId {
    NodeId::from_u128(id)
}

pub(crate) const fn s(id: u128) -> CompoundId {
    CompoundId::from_u128(id)
}

/// The funcs above, each numbering its ports from 1.
pub(crate) fn library() -> Library {
    let mut library = Library::default();
    library.add(
        internals::stub_func(SRC, "src")
            .pure()
            .output(1, OutputDecl::new("value", DataType::Int))
            .build(),
    );
    library.add(
        internals::stub_func(ADD, "add")
            .pure()
            .input(1, InputDecl::required("a", DataType::Int))
            .input(2, InputDecl::required("b", DataType::Int))
            .output(1, OutputDecl::new("sum", DataType::Int))
            .build(),
    );
    library.add(
        internals::stub_func(SINK, "sink")
            .sink()
            .input(1, InputDecl::required("value", DataType::Int))
            .build(),
    );
    library.add(
        internals::stub_func(TICKER, "ticker")
            .event(1, "tick", internals::stub_event())
            .build(),
    );
    library.add(
        internals::stub_func(ANY_SINK, "any sink")
            .sink()
            .input(1, InputDecl::required("value", DataType::Any))
            .build(),
    );
    library.add(
        internals::stub_func(CONFIG_SINK, "config sink")
            .sink()
            .input(1, InputDecl::required("value", DataType::Int).const_only())
            .build(),
    );
    library
}

/// A node of `func_id`, which `library` holds.
pub(crate) fn node(library: &Library, func_id: FuncId) -> Node {
    Node::from(
        library
            .by_id(func_id)
            .expect("the fixture library holds the func"),
    )
}
