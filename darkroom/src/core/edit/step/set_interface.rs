//! [`SetInterface`]: an edit of a compound's interface, with what it takes out of every view.

use std::collections::BTreeMap;

use scenarium::Interface;
use scenarium::{Binding, ChangeSide, CompoundId, GraphId, InterfaceChange, Library, RefusedFate};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::wire_key::{DataWire, WireKey};
use crate::core::edit::edit_facts::{EditFacts, Touched};
use crate::core::edit::step::change::Direction;
use crate::core::edit::step::reversible::Reversible;
use crate::core::edit::step::set_ties::{SetTies, TiesDraft};

/// An edit of one compound's interface: scenarium's [`InterfaceChange`], which holds both
/// interfaces and each binding the new one does not hold in any graph, and in each graph's view
/// the route of each wire it drops and each tie only those wires held. A wire it keeps as a
/// ghost keeps its route, as it is the same wire.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetInterface {
    change: InterfaceChange,
    views: Vec<ViewTies>,
}

/// The ties an interface edit changes in one graph's view.
#[derive(Debug, Serialize, Deserialize)]
struct ViewTies {
    graph: GraphId,
    ties: SetTies,
}

impl SetInterface {
    /// Set compound `id`'s interface in `document` to `to`, planned against `library`, what it
    /// refuses dropped or kept as ghosts as `fate` says.
    ///
    /// # Panics
    /// If the document holds no compound `id`.
    pub(crate) fn plan(
        document: &Document,
        id: CompoundId,
        to: Interface,
        library: &Library,
        fate: RefusedFate,
    ) -> Self {
        let change = document.graphs.plan_interface(id, to, library, fate);
        let mut drafts: BTreeMap<GraphId, TiesDraft<'_>> = BTreeMap::new();
        let dropped = change
            .refused()
            .iter()
            .filter(|_| fate == RefusedFate::Drop);
        for dropped in dropped {
            if let Binding::Wire(output) = dropped.binding() {
                drafts
                    .entry(dropped.graph)
                    .or_insert_with(|| TiesDraft::new(document.view(dropped.graph)))
                    .release([WireKey::Data(DataWire {
                        output,
                        input: dropped.port,
                    })]);
            }
        }
        if fate == RefusedFate::Drop {
            for dropped in change.refused_subscriptions() {
                drafts
                    .entry(dropped.graph)
                    .or_insert_with(|| TiesDraft::new(document.view(dropped.graph)))
                    .release([WireKey::Event(dropped.subscription)]);
            }
        }
        let views = drafts
            .into_iter()
            .map(|(graph, draft)| ViewTies {
                graph,
                ties: draft.finish(),
            })
            .filter(|view| !view.ties.is_noop())
            .collect();
        Self { change, views }
    }

    /// Whether the edit changes nothing: the same interface, which drops nothing.
    pub(crate) fn is_noop(&self) -> bool {
        self.change.before() == self.change.after()
    }

    pub(super) fn write(&self, document: &mut Document, dir: Direction) {
        let side = match dir {
            Direction::Forward => ChangeSide::After,
            Direction::Backward => ChangeSide::Before,
        };
        document.graphs.write_interface(&self.change, side);
        for view in &self.views {
            view.ties.write(document, view.graph, dir);
        }
    }

    /// The ports of every instance and both boundary nodes change, in every graph that places
    /// one, and a refused constant takes its editor with it.
    pub(super) fn facts(&self) -> EditFacts {
        let refused = self.change.refused();
        let count = refused.len() + self.change.refused_subscriptions().len();
        let (dropped, ghosted) = match self.change.fate() {
            RefusedFate::Drop => (count, 0),
            RefusedFate::Ghost => (0, count),
        };
        EditFacts {
            ports: true,
            binding_kind: refused
                .iter()
                .any(|refused| matches!(refused.binding(), Binding::Const(_))),
            dropped,
            ghosted,
            touched: Touched::Several,
            ..EditFacts::NONE
        }
    }
}
