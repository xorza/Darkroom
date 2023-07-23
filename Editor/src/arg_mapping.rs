use egui_node_graph as eng;
use graph_lib::graph::NodeId;

#[derive(Debug, Eq, PartialEq, Clone)]
pub(crate) struct ArgAddress {
    pub(crate) node_id: NodeId,
    pub(crate) index: u32,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ArgMapping {
    // @formatter:off
    triggers: Vec<( eng::InputId,     NodeId)>,
    inputs  : Vec<( eng::InputId, ArgAddress)>,
    events  : Vec<(eng::OutputId, ArgAddress)>,
    outputs : Vec<(eng::OutputId, ArgAddress)>,
    // @formatter:on
}

pub(crate) enum FindByInputIdResult {
    Input(ArgAddress),
    Trigger(NodeId),
}
pub(crate) enum FindByOutputIdResult {
    Output(ArgAddress),
    Event(ArgAddress),
}

impl ArgMapping {
    pub(crate) fn find_input_id(&self, node_id: &NodeId, index: u32) -> eng::InputId {
        self.inputs
            .iter()
            .find_map(|(input_id, aa)|
                if aa.node_id == *node_id && aa.index == index {
                    Some(*input_id)
                } else {
                    None
                }
            )
            .unwrap()
    }
    pub(crate) fn find_output_id(&self, node_id: &NodeId, index: u32) -> eng::OutputId {
        self.outputs
            .iter()
            .find_map(|(output_id, aa)|
                if aa.node_id == *node_id && aa.index == index {
                    Some(*output_id)
                } else {
                    None
                }
            )
            .unwrap()
    }
    pub(crate) fn find_trigger_id(&self, node_id: &NodeId) -> eng::InputId {
        self.triggers
            .iter()
            .find_map(|(input_id, trigger_node_id)|
                if *trigger_node_id == *node_id {
                    Some(*input_id)
                } else {
                    None
                }
            )
            .unwrap()
    }
    pub(crate) fn find_event_id(&self, node_id: &NodeId, index: u32) -> eng::OutputId {
        self.events
            .iter()
            .find_map(|(output_id, aa)|
                if aa.node_id == *node_id && aa.index == index {
                    Some(*output_id)
                } else {
                    None
                }
            )
            .unwrap()
    }

    pub(crate) fn find_input_address(&self, input_id: eng::InputId) -> ArgAddress {
        self.inputs
            .iter()
            .find_map(|(ii, aa)|
                if *ii == input_id {
                    Some(aa.clone())
                } else {
                    None
                }
            )
            .unwrap()
    }
    pub(crate) fn find_output_address(&self, output_id: eng::OutputId) -> ArgAddress {
        self.outputs
            .iter()
            .find_map(|(oi, aa)|
                if *oi == output_id {
                    Some(aa.clone())
                } else {
                    None
                }
            )
            .unwrap()
    }
    pub(crate) fn find_trigger_node_id(&self, input_id: eng::InputId) -> NodeId {
        self.triggers
            .iter()
            .find_map(|(ii, trigger_node_id)|
                if *ii == input_id {
                    Some(*trigger_node_id)
                } else {
                    None
                }
            )
            .unwrap()
    }
    pub(crate) fn find_event_address(&self, output_id: eng::OutputId) -> ArgAddress {
        self.events
            .iter()
            .find_map(|(oi, aa)|
                if *oi == output_id {
                    Some(aa.clone())
                } else {
                    None
                }
            )
            .unwrap()
    }

    pub(crate) fn find_by_input_id(&self, input_id: eng::InputId) -> FindByInputIdResult {
        let result = self.inputs
            .iter()
            .find_map(|(aa_input_id, aa)|
                if *aa_input_id == input_id {
                    Some(FindByInputIdResult::Input(aa.clone()))
                } else {
                    None
                }
            );
        if let Some(result) = result {
            return result;
        }

        let result = self.triggers
            .iter()
            .find_map(|(aa_input_id, node_id)|
                if *aa_input_id == input_id {
                    Some(FindByInputIdResult::Trigger(*node_id))
                } else {
                    None
                }
            );
        if let Some(result) = result {
            return result;
        } else {
            panic!("Input not found")
        }
    }
    pub(crate) fn find_by_output_id(&self, output_id: eng::OutputId) -> FindByOutputIdResult {
        let result = self.outputs
            .iter()
            .find_map(|(aa_output_id, aa)|
                if *aa_output_id == output_id {
                    Some(FindByOutputIdResult::Output(aa.clone()))
                } else {
                    None
                }
            );
        if let Some(result) = result {
            return result;
        }

        let result = self.events
            .iter()
            .find_map(|(aa_output_id, aa)|
                if *aa_output_id == output_id {
                    Some(FindByOutputIdResult::Event(aa.clone()))
                } else {
                    None
                }
            );
        if let Some(result) = result {
            return result;
        } else {
            panic!("Input not found")
        }
    }


    pub(crate) fn add_input(&mut self, eng_input_id: eng::InputId, node_id: NodeId, index: u32) {
        self.inputs.push((eng_input_id, ArgAddress {
            node_id,
            index,
        }));
    }
    pub(crate) fn add_output(&mut self, eng_output_id: eng::OutputId, node_id: NodeId, index: u32) {
        self.outputs.push((eng_output_id, ArgAddress {
            node_id,
            index,
        }));
    }
    pub(crate) fn add_trigger(&mut self, eng_input_id: eng::InputId, node_id: NodeId) {
        self.triggers.push((eng_input_id, node_id));
    }
    pub(crate) fn add_event(&mut self, eng_output_id: eng::OutputId, node_id: NodeId, index: u32) {
        self.events.push((eng_output_id, ArgAddress {
            node_id,
            index,
        }));
    }
}

