use glam::Vec2;
use scenarium::prelude::{Binding, FuncLib, NodeId};

use crate::model::ViewGraph;

#[derive(Default)]
pub struct Scene {
    pub nodes: Vec<SceneNode>,
    pub connections: Vec<SceneConnection>,
}

pub struct SceneNode {
    pub id: NodeId,
    pub name: String,
    pub pos: Vec2,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

pub struct SceneConnection {
    pub src_node: NodeId,
    pub src_port: usize,
    pub tgt_node: NodeId,
    pub tgt_port: usize,
}

impl Scene {
    pub fn rebuild(&mut self, view_graph: &ViewGraph, func_lib: &FuncLib) {
        self.nodes.clear();
        self.connections.clear();

        for vn in view_graph.view_nodes.iter() {
            let Some(node) = view_graph.graph.by_id(&vn.id) else {
                continue;
            };
            let Some(func) = func_lib.by_id(&node.func_id) else {
                continue;
            };
            self.nodes.push(SceneNode {
                id: vn.id,
                name: node.name.clone(),
                pos: vn.pos,
                inputs: node.inputs.iter().map(|i| i.name.clone()).collect(),
                outputs: func.outputs.iter().map(|o| o.name.clone()).collect(),
            });
        }

        for node in view_graph.graph.iter() {
            for (tgt_port, inp) in node.inputs.iter().enumerate() {
                let Binding::Bind(addr) = &inp.binding else {
                    continue;
                };
                self.connections.push(SceneConnection {
                    src_node: addr.target_id,
                    src_port: addr.port_idx,
                    tgt_node: node.id,
                    tgt_port,
                });
            }
        }
    }
}
