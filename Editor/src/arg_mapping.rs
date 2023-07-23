use egui_node_graph as eng;
use graph_lib::graph::NodeId;

#[derive(Debug, Eq, PartialEq, Clone)]
pub(crate) struct ArgAddress {
    pub(crate) node_id: NodeId,
    pub(crate) index: u32,
}

#[derive(Debug, Eq, PartialEq)]
enum Mapping {
    Trigger {
        eng_input_id: eng::InputId,
        node_id: NodeId,
    },
    Input {
        eng_input_id: eng::InputId,
        arg_address: ArgAddress,
    },
    Event {
        eng_output_id: eng::OutputId,
        arg_address: ArgAddress,
    },
    Output {
        eng_output_id: eng::OutputId,
        arg_address: ArgAddress,
    },
}

#[derive(Debug, Default)]
pub(crate) struct ArgMapping {
    addresses: Vec<Mapping>,
}

impl ArgMapping {
    pub(crate) fn find_input_id(&self, node_id: NodeId, index: u32) -> eng::InputId {
        self.addresses
            .iter()
            .find_map(|map|
                if let Mapping::Input {
                    eng_input_id,
                    arg_address: ArgAddress {
                        node_id: aa_node_id,
                        index: aa_index,
                    },
                } = map {
                    (*aa_index == index && *aa_node_id == node_id)
                        .then(|| *eng_input_id)
                } else {
                    None
                }
            )
            .unwrap()
    }
    pub(crate) fn find_output_id(&self, node_id: NodeId, index: u32) -> eng::OutputId {
        self.addresses
            .iter()
            .find_map(|aa|
                if let Mapping::Output {
                    eng_output_id,
                    arg_address: ArgAddress {
                        node_id: aa_node_id,
                        index: aa_index,
                    },
                } = aa {
                    (*aa_index == index && *aa_node_id == node_id)
                        .then(|| *eng_output_id)
                } else {
                    None
                }
            )
            .unwrap()
    }
    pub(crate) fn find_trigger_id(&self, node_id: NodeId) -> eng::InputId {
        self.addresses
            .iter()
            .find_map(|aa|
                if let Mapping::Trigger {
                    eng_input_id,
                    node_id: aa_node_id,
                } = aa {
                    (*aa_node_id == node_id)
                        .then(|| *eng_input_id)
                } else {
                    None
                }
            )
            .unwrap()
    }
    pub(crate) fn find_event_id(&self, node_id: NodeId, index: u32) -> eng::OutputId {
        self.addresses
            .iter()
            .find_map(|aa|
                if let Mapping::Event {
                    eng_output_id,
                    arg_address: ArgAddress {
                        node_id: aa_node_id,
                        index: aa_index,
                    },
                } = aa {
                    (*aa_index == index && *aa_node_id == node_id)
                        .then(|| *eng_output_id)
                } else {
                    None
                }
            )
            .unwrap()
    }

    pub(crate) fn find_input_address(&self, input_id: eng::InputId) -> ArgAddress {
        self.addresses
            .iter()
            .find_map(|aa|
                if let Mapping::Input {
                    eng_input_id,
                    arg_address,
                } = aa {
                    (*eng_input_id == input_id)
                        .then(|| arg_address.clone())
                } else {
                    None
                }
            )
            .unwrap()
    }
    pub(crate) fn find_output_address(&self, output_id: eng::OutputId) -> ArgAddress {
        self.addresses
            .iter()
            .find_map(|aa|
                if let Mapping::Output {
                    eng_output_id,
                    arg_address,
                } = aa {
                    (*eng_output_id == output_id)
                        .then(|| arg_address.clone())
                } else {
                    None
                }
            )
            .unwrap()
    }
    pub(crate) fn find_trigger_node_id(&self, input_id: eng::InputId) -> NodeId {
        self.addresses
            .iter()
            .find_map(|aa|
                if let Mapping::Trigger {
                    eng_input_id,
                    node_id,
                } = aa {
                    (*eng_input_id == input_id)
                        .then(|| *node_id)
                } else {
                    None
                }
            )
            .unwrap()
    }
    pub(crate) fn find_event_address(&self, output_id: eng::OutputId) -> ArgAddress {
        self.addresses
            .iter()
            .find_map(|aa|
                if let Mapping::Event {
                    eng_output_id,
                    arg_address,
                } = aa {
                    (*eng_output_id == output_id)
                        .then(|| arg_address.clone())
                } else {
                    None
                }
            )
            .unwrap()
    }

    pub(crate) fn add_input(&mut self, eng_input_id: eng::InputId, node_id: NodeId, index: u32) {
        self.addresses.push(Mapping::Input {
            eng_input_id,
            arg_address: ArgAddress {
                node_id,
                index,
            },
        });
    }
    pub(crate) fn add_output(&mut self, eng_output_id: eng::OutputId, node_id: NodeId, index: u32) {
        self.addresses.push(Mapping::Output {
            eng_output_id,
            arg_address: ArgAddress {
                node_id,
                index,
            },
        });
    }
    pub(crate) fn add_trigger(&mut self, eng_input_id: eng::InputId, node_id: NodeId) {
        self.addresses.push(Mapping::Trigger {
            eng_input_id,
            node_id,
        });
    }
    pub(crate) fn add_event(&mut self, eng_output_id: eng::OutputId, node_id: NodeId, index: u32) {
        self.addresses.push(Mapping::Event {
            eng_output_id,
            arg_address: ArgAddress {
                node_id,
                index,
            },
        });
    }
}

