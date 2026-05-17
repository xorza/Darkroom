use glam::Vec2;
use palantir::InternedStr;
use scenarium::prelude::{Binding, NodeId};

use crate::document::Document;

#[derive(Default, Debug)]
pub struct Scene {
    pub nodes: Vec<SceneNode>,
    pub connections: Vec<SceneConnection>,
    /// Flat pool of port-name handles. Each `SceneNode` slices into it
    /// via `inputs` / `outputs` spans — keeps per-node allocations to
    /// zero in steady state.
    pub port_names: Vec<InternedStr>,
}

#[derive(Debug)]
pub struct SceneNode {
    pub id: NodeId,
    pub pos: Vec2,
    pub name: InternedStr,
    pub inputs: PortSpan,
    pub outputs: PortSpan,
}

#[derive(Debug)]
pub struct SceneConnection {
    pub src_node: NodeId,
    pub src_port: usize,
    pub tgt_node: NodeId,
    pub tgt_port: usize,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct PortSpan {
    pub start: u32,
    pub len: u32,
}

impl Scene {
    /// Names live in palantir's per-frame text arena, which clears at
    /// the next `Ui::frame` — so `Scene` must be rebuilt every frame
    /// before any widget consumes it. `App::frame` enforces this.
    pub fn rebuild(&mut self, doc: &Document) {
        let view_graph = &doc.view_graph;
        let func_lib = &doc.func_lib;
        self.nodes.clear();
        self.connections.clear();
        self.port_names.clear();

        for vn in view_graph.view_nodes.iter() {
            let Some(node) = view_graph.graph.by_id(&vn.id) else {
                continue;
            };
            let Some(func) = func_lib.by_id(&node.func_id) else {
                continue;
            };
            let inputs = push_port_names(
                &mut self.port_names,
                node.inputs.iter().map(|i| i.name.as_str()),
            );
            let outputs = push_port_names(
                &mut self.port_names,
                func.outputs.iter().map(|o| o.name.as_str()),
            );
            self.nodes.push(SceneNode {
                id: vn.id,
                pos: vn.pos,
                name: node.name.clone().into(),
                inputs,
                outputs,
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

    pub fn ports(&self, span: PortSpan) -> &[InternedStr] {
        let start = span.start as usize;
        &self.port_names[start..start + span.len as usize]
    }
}

fn push_port_names<'a>(
    pool: &mut Vec<InternedStr>,
    names: impl Iterator<Item = &'a str>,
) -> PortSpan {
    let start = pool.len() as u32;
    let mut len = 0u32;
    for n in names {
        pool.push(n.to_owned().into());
        len += 1;
    }
    PortSpan { start, len }
}
