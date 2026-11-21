//! [`CompoundDigest`]: what a compound's definition is, apart from the id it is held under.
//!
//! - **A digest is of the definition alone**: its name and description, its interface, and its
//!   graph in node id order with its ghosts, each compound an instance places by its own digest.
//!   A ghost node is encoded as it is, a compound it would place by id: it places nothing, and a
//!   loop a load broke is held by one. The id is
//!   left out, so two documents that hold one definition under two ids digest it alike, and a
//!   paste or an update tells an equal definition from a changed one. As Git names a tree by
//!   its content and each subtree by its own hash. Refused: the serialized definition hashed as
//!   it stands, whose node map has no order and whose instances name ids.
//! - **A digest leaves saved declarations out**: what a ghost node declared and the saved port
//!   a ghost holds are a library's words, not the author's, so two copies of a definition saved
//!   against two versions of a library stay equal.

use hashbrown::HashMap;

use serde::{Deserialize, Serialize};

use common::SerdeFormat;

use crate::graph::dfs;
use crate::graph::graph_set::GraphSet;
use crate::graph::identity::CompoundId;
use crate::graph::node::{Node, NodeKind};

/// The blake3 digest of a compound's definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CompoundDigest(#[serde(with = "common::hex_bytes")] [u8; 32]);

impl GraphSet {
    /// The digest of compound `id`'s definition; see the module.
    ///
    /// # Panics
    /// If this set holds no compound `id`.
    pub fn digest(&self, id: CompoundId) -> CompoundDigest {
        assert!(
            self.compound(id).is_some(),
            "a digested compound is in the set"
        );
        // Each compound after every one it places, so its instances read digests already worked
        // out, and a compound placed many times is digested once.
        let walk = dfs::back_edges([id], |owner, next| {
            next.extend(self.placed_compounds(owner).map(|placed| ((), placed)));
        });
        let mut known = HashMap::with_capacity(walk.post_order.len());
        for compound in walk.post_order {
            let digest = self.digest_of(compound, &known);
            known.insert(compound, digest);
        }
        known[&id]
    }

    /// `id`'s digest, each compound it places read from `known`.
    fn digest_of(
        &self,
        id: CompoundId,
        known: &HashMap<CompoundId, CompoundDigest>,
    ) -> CompoundDigest {
        let compound = self
            .compound(id)
            .expect("a digested compound is in the set");
        let mut hasher = blake3::Hasher::new();
        let mut write = |bytes: &[u8]| {
            hasher.update(&(bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        };
        write(compound.name.as_bytes());
        write(&encode(&compound.description));
        write(&encode(compound.interface.ports()));
        let graph = &compound.graph;
        for node in graph.iter() {
            write(&encode(&node.node_id));
            write(&encode_node(&node, known));
        }
        write(&encode(&graph.wiring.bindings));
        write(&encode(&graph.wiring.subscriptions().collect::<Vec<_>>()));
        for (node_id, ghost) in graph.ghost_nodes() {
            write(&encode(&node_id));
            write(&encode(&ghost.reason));
            write(&encode(&ghost.node));
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
        CompoundDigest(*hasher.finalize().as_bytes())
    }
}

/// `node`'s encoding, an instance's compound by that compound's digest rather than its id where
/// `known` holds it; by its id where it does not: a compound the set lacks, or one an instance
/// returns to on the walk's path, which only a loop no load has broken can hold.
fn encode_node(node: &Node, known: &HashMap<CompoundId, CompoundDigest>) -> Vec<u8> {
    match node.kind {
        NodeKind::Instance {
            compound: placed,
            cache,
        } if let Some(digest) = known.get(&placed) => {
            encode(&(digest, cache, &node.name, node.disabled))
        }
        _ => encode(node),
    }
}

/// `value`'s compact binary encoding, one value the same bytes wherever it is encoded.
fn encode<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    SerdeFormat::Bitcode
        .serialize(value)
        .expect("a definition's parts encode")
}
