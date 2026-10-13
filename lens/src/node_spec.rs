//! [`NodeSpec`]: what a library lists a node under.

use scenarium::{Func, FuncId, FuncLambda};

/// The id, name and description a node is listed under, for the node builders that share one
/// shape across many nodes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NodeSpec {
    pub(crate) id: FuncId,
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
}

impl NodeSpec {
    /// The node this spec names, running `lambda`.
    pub(crate) fn func(self, lambda: FuncLambda) -> Func {
        Func::new(self.id, self.name, lambda).description(self.description)
    }
}
