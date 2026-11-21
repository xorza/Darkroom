//! [`UnknownType`]: a type an interface names that the library lacks, and the
//! [`InterfacePort`]s that name it.

use crate::data::data_type::DataType;
use crate::decl::ports::error::PortList;
use crate::identity::{CompoundId, PortId};

/// A type an interface names that the library does not register, and each interface port that
/// names it: what a load reports, as the ports are kept as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownType {
    pub data_type: DataType,
    pub ports: Vec<InterfacePort>,
}

/// One port of a compound's interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterfacePort {
    pub compound: CompoundId,
    /// [`PortList::Inputs`] or [`PortList::Outputs`]: an event carries no type to be unknown.
    pub list: PortList,
    pub port: PortId,
}
