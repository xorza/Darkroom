//! [`SubgraphDigest`]: what a subgraph's definition is, apart from the id it is held under.
//!
//! - **A digest is of the definition alone**: its name and description, its interface, and its
//!   graph in node id order with its ghosts, each subgraph it places by its own digest. The id is
//!   left out, so two documents that hold one definition under two ids digest it alike, and a
//!   paste or an update tells an equal definition from a changed one. As Git names a tree by
//!   its content and each subtree by its own hash. Refused: the serialized definition hashed as
//!   it stands, whose node map has no order and whose instances name ids.
//! - **A digest leaves saved declarations out**: what a ghost node declared and the saved port
//!   a ghost holds are a library's words, not the author's, so two copies of a definition saved
//!   against two versions of a library stay equal.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use common::SerdeFormat;

use crate::graph::graph_set::GraphSet;
use crate::graph::identity::SubgraphId;
use crate::graph::node::{Node, NodeKind};

/// The blake3 digest of a subgraph's definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubgraphDigest(#[serde(with = "common::hex_bytes")] [u8; 32]);

impl GraphSet {
    /// The digest of subgraph `id`'s definition; see the module.
    ///
    /// # Panics
    /// If this set holds no subgraph `id`.
    pub fn digest(&self, id: SubgraphId) -> SubgraphDigest {
        self.digest_with(id, &mut HashMap::new())
    }

    /// [`Self::digest`], reusing each placed subgraph's digest once worked out: a subgraph placed
    /// many times is digested once. The recursion is bounded by the set's depth gate.
    fn digest_with(
        &self,
        id: SubgraphId,
        known: &mut HashMap<SubgraphId, SubgraphDigest>,
    ) -> SubgraphDigest {
        if let Some(&digest) = known.get(&id) {
            return digest;
        }
        let subgraph = self
            .subgraph(id)
            .expect("a digested subgraph is in the set");
        let mut hasher = blake3::Hasher::new();
        let mut write = |bytes: &[u8]| {
            hasher.update(&(bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        };
        write(subgraph.name.as_bytes());
        write(&encode(&subgraph.description));
        write(&encode(subgraph.interface.ports()));
        let graph = &subgraph.graph;
        let mut nodes: Vec<_> = graph.iter().collect();
        nodes.sort_unstable_by_key(|node| node.node_id);
        for node in nodes {
            write(&encode(&node.node_id));
            write(&self.encode_node(&node, known));
        }
        write(&encode(&graph.bindings));
        write(&encode(&graph.subscriptions().collect::<Vec<_>>()));
        for (node_id, ghost) in graph.ghost_nodes() {
            write(&encode(&node_id));
            write(&encode(&ghost.reason));
            write(&self.encode_node(&ghost.node, known));
        }
        let ghosts = graph.ghost_wiring();
        for (port, ghost) in ghosts.bindings() {
            write(&encode(&(port, ghost.binding())));
        }
        write(&encode(
            &ghosts
                .subscriptions()
                .map(|(subscription, _)| subscription)
                .collect::<Vec<_>>(),
        ));
        let digest = SubgraphDigest(*hasher.finalize().as_bytes());
        known.insert(id, digest);
        digest
    }

    /// `node`'s encoding, an instance's subgraph by that subgraph's digest rather than its id;
    /// an instance of a subgraph the set lacks, as a ghost node may be, by its id.
    fn encode_node(&self, node: &Node, known: &mut HashMap<SubgraphId, SubgraphDigest>) -> Vec<u8> {
        match node.kind {
            NodeKind::Instance {
                subgraph: placed,
                cache,
            } if self.subgraph(placed).is_some() => {
                let digest = self.digest_with(placed, known);
                encode(&(digest, cache, &node.name, node.disabled))
            }
            _ => encode(node),
        }
    }
}

/// `value`'s compact binary encoding, one value the same bytes wherever it is encoded.
fn encode<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    SerdeFormat::Bitcode
        .serialize(value)
        .expect("a definition's parts encode")
}
