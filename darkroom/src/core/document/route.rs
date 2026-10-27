//! [`Route`]: the ties one wire runs through.

use std::slice;

use serde::{Deserialize, Serialize};

use crate::core::document::error::RouteError;
use crate::core::document::tie::TieId;

/// The ties a wire runs through, in order from its output end to its input end. Never empty, and
/// never names a tie twice. Stored rather than derived, as the order of the ties is the wire's own;
/// a strand's slot in each band is derived each frame. `darkroom/AGENTS.md`, "Wire ties", "A route
/// is stored, a strand's slot is derived", holds the decision.
///
/// A file can hold a route that breaks the rules: the load repairs it, and
/// [`GraphView::validate`](crate::core::document::graph_view::GraphView::validate) checks it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Route(Vec<TieId>);

impl Route {
    /// The route through `ties`.
    ///
    /// # Panics
    ///
    /// When `ties` is empty or names a tie twice.
    pub(crate) fn new(ties: impl IntoIterator<Item = TieId>) -> Self {
        Self::checked(ties).expect("a route names at least one tie, each once")
    }

    /// The route through `ties`, or the rule they break.
    pub(crate) fn checked(ties: impl IntoIterator<Item = TieId>) -> Result<Self, RouteError> {
        let route = Self(ties.into_iter().collect());
        route.check()?;
        Ok(route)
    }

    /// The rule this route breaks, if any.
    pub(crate) fn check(&self) -> Result<(), RouteError> {
        if self.0.is_empty() {
            return Err(RouteError::Empty);
        }
        // Routes hold a few ties, so a scan of the earlier ones beats a set.
        for (idx, &tie) in self.0.iter().enumerate() {
            if self.0[..idx].contains(&tie) {
                return Err(RouteError::RepeatedTie { tie });
            }
        }
        Ok(())
    }

    /// The ties, from the output end to the input end.
    pub(crate) fn iter(&self) -> slice::Iter<'_, TieId> {
        self.0.iter()
    }

    pub(crate) fn contains(&self, tie: TieId) -> bool {
        self.0.contains(&tie)
    }

    /// This route with `tie` taken out, or `None` when no tie is left.
    pub(crate) fn without(&self, tie: TieId) -> Option<Self> {
        let rest: Vec<TieId> = self.0.iter().copied().filter(|&step| step != tie).collect();
        (!rest.is_empty()).then_some(Self(rest))
    }
}

impl<'a> IntoIterator for &'a Route {
    type Item = &'a TieId;
    type IntoIter = slice::Iter<'a, TieId>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::core::document::route::Route;
    use crate::core::document::tie::TieId;

    impl Route {
        /// A route that may break the rules, as a file can hold one.
        pub(crate) fn unchecked(ties: impl IntoIterator<Item = TieId>) -> Self {
            Self(ties.into_iter().collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::core::document::error::RouteError;
    use crate::core::document::route::Route;
    use crate::core::document::tie::TieId;

    /// One tie or several distinct ones make a route; no tie, or a tie named a second time,
    /// does not, and the error names the repeated tie.
    #[test]
    fn a_route_names_at_least_one_tie_each_once() {
        let [a, b] = [1, 2].map(TieId::from_u128);
        assert_eq!(
            Route::checked([a]).unwrap().iter().collect::<Vec<_>>(),
            [&a]
        );
        assert_eq!(
            Route::new([b, a]).iter().copied().collect::<Vec<_>>(),
            [b, a],
            "the order is the route's own"
        );
        assert_eq!(Route::checked([]), Err(RouteError::Empty));
        assert_eq!(
            Route::checked([a, b, a]),
            Err(RouteError::RepeatedTie { tie: a })
        );
    }
}
