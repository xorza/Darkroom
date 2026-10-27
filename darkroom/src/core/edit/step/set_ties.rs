//! [`SetTies`]: wire ties and the routes through them, before and after.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::graph_view::GraphView;
use crate::core::document::item_id::ItemId;
use crate::core::document::route::Route;
use crate::core::document::tie::{Tie, TieId};
use crate::core::document::wire_key::WireKey;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;

/// Each tie and each route an edit changes, before and after: `None` is a tie or a route the
/// document does not hold. One step kind for every tie edit, adding or removing a tie, turning
/// it, tying a wire or releasing one, and the part of a wiring edit that ends a tied wire. Built
/// through a [`TiesDraft`], which removes each tie the edit leaves with no wire.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SetTies {
    ties: Vec<TieChange>,
    routes: Vec<RouteChange>,
}

/// One tie, before and after.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct TieChange {
    tie: TieId,
    placed: Change<Option<Tie>>,
    /// Whether the tie was selected: a removal takes it out of the selection, and its revert
    /// puts it back.
    selected: bool,
}

/// One wire's route, before and after.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct RouteChange {
    wire: WireKey,
    route: Change<Option<Route>>,
}

impl SetTies {
    /// Fold `next`, a later frame of the same held gesture, into this step:
    /// keep each tie's and route's `from` and adopt its latest `to`, and take
    /// any `next` changes that this step does not hold whole.
    pub(crate) fn absorb(&mut self, next: &Self) {
        for change in &next.ties {
            match self.ties.iter_mut().find(|open| open.tie == change.tie) {
                Some(open) => open.placed.to = change.placed.to,
                None => self.ties.push(TieChange {
                    tie: change.tie,
                    placed: change.placed.clone(),
                    selected: change.selected,
                }),
            }
        }
        for change in &next.routes {
            match self.routes.iter_mut().find(|open| open.wire == change.wire) {
                Some(open) => open.route.to.clone_from(&change.route.to),
                None => self.routes.push(RouteChange {
                    wire: change.wire,
                    route: change.route.clone(),
                }),
            }
        }
    }
}

impl Reversible for SetTies {
    fn write(&self, document: &mut Document, dir: Direction) {
        let view = &mut document.main_view;
        for change in &self.routes {
            match change.route.half(dir) {
                Some(route) => view.routes.insert(change.wire, route.clone()),
                None => view.routes.remove(&change.wire),
            };
        }
        for change in &self.ties {
            let item = ItemId::Tie(change.tie);
            if let Some(tie) = change.placed.half(dir) {
                view.ties.insert(change.tie, *tie);
                if change.selected && change.placed.half(dir.reversed()).is_none() {
                    view.selected.insert(item);
                }
            } else {
                view.ties.remove(&change.tie);
                view.selected.remove(&item);
            }
        }
    }

    fn is_noop(&self) -> bool {
        self.ties.iter().all(|change| change.placed.unchanged())
            && self.routes.iter().all(|change| change.route.unchanged())
    }

    /// Ties are saved layout, as node positions are.
    fn dirties_document(&self) -> bool {
        true
    }

    /// No node remeasures: a tie paints between bodies that keep their size.
    fn invalidates_cached_geometry(&self) -> bool {
        false
    }

    fn changes_node_set(&self) -> bool {
        false
    }
}

/// The ties and routes an edit leaves, over the view it edits: each change is written here, and
/// [`Self::finish`] reads what each held before out of the view.
#[derive(Debug)]
pub(crate) struct TiesDraft<'a> {
    view: &'a GraphView,
    ties: BTreeMap<TieId, Option<Tie>>,
    routes: BTreeMap<WireKey, Option<Route>>,
}

impl<'a> TiesDraft<'a> {
    pub(crate) const fn new(view: &'a GraphView) -> Self {
        Self {
            view,
            ties: BTreeMap::new(),
            routes: BTreeMap::new(),
        }
    }

    /// The route of `wire` as the draft leaves it.
    pub(crate) fn route(&self, wire: WireKey) -> Option<&Route> {
        match self.routes.get(&wire) {
            Some(route) => route.as_ref(),
            None => self.view.routes.get(&wire),
        }
    }

    pub(crate) fn set_tie(&mut self, tie: TieId, to: Option<Tie>) {
        self.ties.insert(tie, to);
    }

    pub(crate) fn set_route(&mut self, wire: WireKey, to: Option<Route>) {
        self.routes.insert(wire, to);
    }

    /// End the route of each of `wires`: a wire the edit ends or rewires keeps no route.
    pub(crate) fn release(&mut self, wires: impl IntoIterator<Item = WireKey>) {
        for wire in wires {
            if self.route(wire).is_some() {
                self.set_route(wire, None);
            }
        }
    }

    /// Take `wire` out of `tie`: its route goes on without it, or goes when it held only `tie`.
    pub(crate) fn leave(&mut self, wire: WireKey, tie: TieId) {
        let to = self
            .route(wire)
            .expect("a wire leaves a tie it runs through")
            .without(tie);
        self.set_route(wire, to);
    }

    /// Remove `tie`, and take every wire through it out of it.
    pub(crate) fn remove_tie(&mut self, tie: TieId) {
        let through: BTreeSet<WireKey> = self
            .view
            .routes
            .keys()
            .chain(self.routes.keys())
            .copied()
            .filter(|&wire| self.route(wire).is_some_and(|route| route.contains(tie)))
            .collect();
        for wire in through {
            self.leave(wire, tie);
        }
        self.set_tie(tie, None);
    }

    /// The step that makes the view what this draft leaves, with each tie that a changed route
    /// named before and no route names after removed too, as a tie holds at least one wire.
    pub(crate) fn finish(mut self) -> SetTies {
        let named_before: BTreeSet<TieId> = self
            .routes
            .keys()
            .filter_map(|wire| self.view.routes.get(wire))
            .flatten()
            .copied()
            .collect();
        for tie in named_before {
            if !self.ties.contains_key(&tie) && !self.routed(tie) {
                self.set_tie(tie, None);
            }
        }
        let view = self.view;
        SetTies {
            ties: self
                .ties
                .into_iter()
                .map(|(tie, to)| TieChange {
                    tie,
                    placed: Change {
                        from: view.ties.get(&tie).copied(),
                        to,
                    },
                    selected: view.selected.contains(&ItemId::Tie(tie)),
                })
                .collect(),
            routes: self
                .routes
                .into_iter()
                .map(|(wire, to)| RouteChange {
                    wire,
                    route: Change {
                        from: view.routes.get(&wire).cloned(),
                        to,
                    },
                })
                .collect(),
        }
    }

    /// Whether a route the draft leaves names `tie`.
    fn routed(&self, tie: TieId) -> bool {
        let kept = self
            .view
            .routes
            .iter()
            .filter(|(wire, _)| !self.routes.contains_key(wire))
            .map(|(_, route)| route);
        kept.chain(self.routes.values().flatten())
            .any(|route| route.contains(tie))
    }
}
