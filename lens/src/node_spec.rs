//! [`NodeSpec`]: what a library lists a node under.

use scenarium::{Func, FuncBuilder, FuncId, FuncLambda};

/// The id, name and description a node is listed under, for the node builders that share one
/// shape across many nodes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NodeSpec {
    pub(crate) id: FuncId,
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
}

impl NodeSpec {
    /// The declaration of the node this spec names, running `lambda`, for the
    /// caller to finish and build.
    pub(crate) fn builder(self, lambda: FuncLambda) -> FuncBuilder {
        Func::builder(self.id, self.name, lambda).description(self.description)
    }
}
