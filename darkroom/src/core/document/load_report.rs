//! [`LoadReport`]: what a load changed to make a decoded document consistent
//! with itself and the library.

use std::fmt;
use std::fmt::{Display, Formatter};

use scenarium::{GraphId, NodeAddr, NodeId, ProgramId, Repair, SubgraphId, UnknownType};

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
    /// Each type an interface names that this build's library does not register, kept as
    /// written. No change of the document, so it leaves the document clean.
    pub(crate) unknown_types: Vec<UnknownType>,
}

/// A [`scenarium::PathForm::Foreign`] path, which a load and a save keep byte for byte. No
/// change of the document, so it leaves the document clean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeignPath {
    pub(crate) node: NodeAddr,
    pub(crate) path: String,
}

/// One change a load made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DocumentRepair {
    /// A change to one graph: its wiring, a stub attached or a node kept as one.
    Graph { graph: GraphId, repair: Repair },
    /// A change to one graph's view.
    View { graph: GraphId, repair: ViewRepair },
    /// A subgraph with no side, given one with its nodes placed in a row.
    SideAdded { subgraph: SubgraphId },
    /// A side for no subgraph.
    SideDropped { subgraph: SubgraphId },
    /// A viewer tab on a node that is not a preview node in the graph.
    ViewerClosed { program_id: ProgramId },
    /// An inspector tab pinned to a node the graph does not hold.
    InspectorClosed { node_id: NodeId },
}

/// One change a load made to a graph's view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ViewRepair {
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
}

impl LoadReport {
    pub(crate) const fn is_empty(&self) -> bool {
        self.repairs.is_empty() && self.foreign_paths.is_empty() && self.unknown_types.is_empty()
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
        for ForeignPath { node, path } in &self.foreign_paths {
            tracing::warn!(
                target: "darkroom::load",
                "{node:?} names {path:?}, a path this system cannot read as written, kept as it is"
            );
        }
        for UnknownType { data_type, ports } in &self.unknown_types {
            tracing::warn!(
                target: "darkroom::load",
                "{data_type:?} is no type this build registers, kept as it is on {ports:?}"
            );
        }
        status.warn(StatusFamily::Document, self.to_string());
    }
}

/// The status line of a load with something to report: how many changes it made, how many paths
/// the document names that this system cannot read as written, and how many types it names that
/// this build does not register.
impl Display for LoadReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let parts = [
            (
                self.repairs.len(),
                "the load made ",
                " repair(s) to the document; save to keep them",
            ),
            (
                self.foreign_paths.len(),
                "the document names ",
                " path(s) this system cannot read as written",
            ),
            (
                self.unknown_types.len(),
                "the document names ",
                " type(s) this build does not register",
            ),
        ];
        let mut separator = "";
        for (count, before, after) in parts {
            if count > 0 {
                write!(f, "{separator}{before}{count}{after}")?;
                separator = "; ";
            }
        }
        Ok(())
    }
}

impl Display for DocumentRepair {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Graph {
                graph: GraphId::Main,
                repair,
            } => repair.fmt(f),
            Self::Graph {
                graph: GraphId::Subgraph(id),
                repair,
            } => write!(f, "in subgraph {id:?}, {repair}"),
            Self::View {
                graph: GraphId::Main,
                repair,
            } => repair.fmt(f),
            Self::View {
                graph: GraphId::Subgraph(id),
                repair,
            } => write!(f, "in the view of subgraph {id:?}, {repair}"),
            Self::SideAdded { subgraph } => {
                write!(
                    f,
                    "placed the nodes of subgraph {subgraph:?} in a row: it had no view"
                )
            }
            Self::SideDropped { subgraph } => {
                write!(
                    f,
                    "removed the view of {subgraph:?}, which names no subgraph"
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

impl Display for ViewRepair {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
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
        }
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{
        CustomTypeId, DataType, GraphId, InterfacePort, NodeAddr, NodeId, PortId, PortList,
        SubgraphId, UnknownType,
    };

    use crate::core::document::load_report::{DocumentRepair, ForeignPath, LoadReport, ViewRepair};

    /// The status line names what the load has to report, and each part alone reads whole.
    #[test]
    fn the_status_line_names_each_kind() {
        let repairs = vec![DocumentRepair::View {
            graph: GraphId::Main,
            repair: ViewRepair::ViewportReset,
        }];
        let foreign_paths: Vec<ForeignPath> = ["/a", "/b"]
            .map(|path| ForeignPath {
                node: NodeAddr::new(GraphId::Main, NodeId::unique()),
                path: path.to_string(),
            })
            .into();
        let unknown_types = vec![UnknownType {
            data_type: DataType::Custom(CustomTypeId::from_u128(1)),
            ports: vec![InterfacePort {
                subgraph: SubgraphId::from_u128(1),
                list: PortList::Inputs,
                port: PortId::from_u128(1),
            }],
        }];
        assert!(LoadReport::default().is_empty());
        let report = |repairs: &[DocumentRepair], paths: &[ForeignPath], types: &[UnknownType]| {
            let report = LoadReport {
                repairs: repairs.to_vec(),
                foreign_paths: paths.to_vec(),
                unknown_types: types.to_vec(),
            };
            assert!(!report.is_empty());
            report.to_string()
        };
        assert_eq!(
            report(&repairs, &[], &[]),
            "the load made 1 repair(s) to the document; save to keep them"
        );
        assert_eq!(
            report(&[], &foreign_paths, &[]),
            "the document names 2 path(s) this system cannot read as written"
        );
        assert_eq!(
            report(&[], &[], &unknown_types),
            "the document names 1 type(s) this build does not register"
        );
        assert_eq!(
            report(&repairs, &foreign_paths, &unknown_types),
            "the load made 1 repair(s) to the document; save to keep them; the document names 2 \
             path(s) this system cannot read as written; the document names 1 type(s) this build \
             does not register"
        );
    }
}
