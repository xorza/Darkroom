//! [`LoadReport`]: what a load changed to make a decoded document consistent
//! with itself and the library.

use std::fmt;
use std::fmt::{Display, Formatter};

use scenarium::{NodeId, ProgramId, Repair};

use crate::core::document::item_id::ItemId;
use crate::core::document::tie::TieId;
use crate::core::document::wire_key::WireKey;
use crate::core::status_log::{StatusFamily, StatusLog};

/// Every change a load made, in the order it made them, and what it kept but could not read here.
/// Empty for a document that was already consistent.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LoadReport {
    pub(crate) repairs: Vec<DocumentRepair>,
    pub(crate) foreign_paths: Vec<ForeignPath>,
}

/// A [`scenarium::PathForm::Foreign`] path, which a load and a save keep byte for byte. No
/// change of the document, so it leaves the document clean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeignPath {
    pub(crate) node_id: NodeId,
    pub(crate) path: String,
}

/// One change a load made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DocumentRepair {
    /// A graph repair: wiring, a node the library cannot hold, or a stub it can again.
    Graph(Repair),
    ViewportReset,
    /// A placement for no node or stub.
    PlacementDropped {
        node_id: NodeId,
    },
    /// A non-finite position, moved to the origin.
    PosReset {
        node_id: NodeId,
    },
    /// A node or stub with no placement, placed at the origin.
    Placed {
        node_id: NodeId,
    },
    /// The route of a wire the graph does not hold.
    RouteDropped {
        wire: WireKey,
    },
    /// A route's step that names no tie.
    RouteStepDropped {
        wire: WireKey,
        tie: TieId,
    },
    /// A route's step that names a tie an earlier step names.
    RouteStepRepeated {
        wire: WireKey,
        tie: TieId,
    },
    /// A route with no step left, or none to start with.
    EmptyRouteDropped {
        wire: WireKey,
    },
    /// A tie that no route names.
    TieDropped {
        tie: TieId,
    },
    /// A tie at a non-finite position, moved to the origin.
    TiePosReset {
        tie: TieId,
    },
    /// A tie pinned to an angle outside `[0, 360)`, set to turn itself.
    TieAngleFreed {
        tie: TieId,
    },
    /// A selection entry the view does not hold.
    SelectionDropped {
        item: ItemId,
    },
    /// A viewer tab on a node that is not a preview node in the graph.
    ViewerClosed {
        program_id: ProgramId,
    },
    /// An inspector tab pinned to a node the graph does not hold.
    InspectorClosed {
        node_id: NodeId,
    },
}

impl LoadReport {
    pub(crate) const fn is_empty(&self) -> bool {
        self.repairs.is_empty() && self.foreign_paths.is_empty()
    }

    /// Log each change and each foreign path, and put one line on the status bar that says how
    /// many there were. The next save clears the line: a save accepts the changes.
    pub(crate) fn report(&self, status: &mut StatusLog) {
        if self.is_empty() {
            return;
        }
        for repair in &self.repairs {
            tracing::warn!(target: "darkroom::load", "{repair}");
        }
        for ForeignPath { node_id, path } in &self.foreign_paths {
            tracing::warn!(
                target: "darkroom::load",
                "{node_id:?} names {path:?}, a path this system cannot read as written, kept as it is"
            );
        }
        status.warn(StatusFamily::Document, self.to_string());
    }
}

/// The status line of a load with something to report: how many changes it made, and how many
/// paths the document names that this system cannot read as written.
impl Display for LoadReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if !self.repairs.is_empty() {
            write!(
                f,
                "the load made {} repair(s) to the document; save to keep them",
                self.repairs.len()
            )?;
        }
        if !self.foreign_paths.is_empty() {
            if !self.repairs.is_empty() {
                f.write_str("; ")?;
            }
            write!(
                f,
                "the document names {} path(s) this system cannot read as written",
                self.foreign_paths.len()
            )?;
        }
        Ok(())
    }
}

impl Display for DocumentRepair {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Graph(repair) => repair.fmt(f),
            Self::ViewportReset => f.write_str("reset the viewport"),
            Self::PlacementDropped { node_id } => {
                write!(
                    f,
                    "removed the placement of {node_id:?}, which names no node"
                )
            }
            Self::PosReset { node_id } => {
                write!(
                    f,
                    "moved {node_id:?} to the origin: its position is not finite"
                )
            }
            Self::Placed { node_id } => {
                write!(f, "placed {node_id:?} at the origin: it had no placement")
            }
            Self::RouteDropped { wire } => {
                write!(f, "removed the route of {wire:?}, which names no wire")
            }
            Self::RouteStepDropped { wire, tie } => {
                write!(
                    f,
                    "removed {tie:?} from the route of {wire:?}: it names no tie"
                )
            }
            Self::RouteStepRepeated { wire, tie } => {
                write!(f, "removed a repeat of {tie:?} from the route of {wire:?}")
            }
            Self::EmptyRouteDropped { wire } => {
                write!(f, "removed the route of {wire:?}: it names no tie")
            }
            Self::TieDropped { tie } => {
                write!(f, "removed the tie {tie:?}, which holds no wire")
            }
            Self::TiePosReset { tie } => {
                write!(
                    f,
                    "moved the tie {tie:?} to the origin: its position is not finite"
                )
            }
            Self::TieAngleFreed { tie } => {
                write!(
                    f,
                    "freed the angle of the tie {tie:?}: it is not in [0, 360)"
                )
            }
            Self::SelectionDropped { item } => {
                write!(
                    f,
                    "removed {item:?} from the selection: the view does not hold it"
                )
            }
            Self::InspectorClosed { node_id } => {
                write!(f, "closed the inspector of {node_id:?}, which is no node")
            }
            Self::ViewerClosed { program_id } => {
                write!(
                    f,
                    "closed the viewer of {program_id:?}, which is no preview node"
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use scenarium::NodeId;

    use crate::core::document::load_report::{DocumentRepair, ForeignPath, LoadReport};

    /// The status line names what the load has to report, and each part alone reads whole.
    #[test]
    fn the_status_line_names_each_kind() {
        let repairs = vec![DocumentRepair::ViewportReset];
        let foreign_paths = vec![
            ForeignPath {
                node_id: NodeId::unique(),
                path: "/a".to_string(),
            },
            ForeignPath {
                node_id: NodeId::unique(),
                path: "/b".to_string(),
            },
        ];
        assert!(LoadReport::default().is_empty());
        let only_repairs = LoadReport {
            repairs: repairs.clone(),
            ..LoadReport::default()
        };
        assert!(!only_repairs.is_empty());
        assert_eq!(
            only_repairs.to_string(),
            "the load made 1 repair(s) to the document; save to keep them"
        );
        let only_paths = LoadReport {
            foreign_paths: foreign_paths.clone(),
            ..LoadReport::default()
        };
        assert!(!only_paths.is_empty());
        assert_eq!(
            only_paths.to_string(),
            "the document names 2 path(s) this system cannot read as written"
        );
        assert_eq!(
            LoadReport {
                repairs,
                foreign_paths,
            }
            .to_string(),
            "the load made 1 repair(s) to the document; save to keep them; the document names 2 \
             path(s) this system cannot read as written"
        );
    }
}
