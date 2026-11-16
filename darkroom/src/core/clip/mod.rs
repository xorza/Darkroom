//! [`Clip`]: a document fragment as the system clipboard carries it, text under a header line.
//!
//! - **Copy and paste go through the system clipboard as text**: the nodes, the wires among them,
//!   their ties and places, and each subgraph they place with its view, in RON under a header
//!   line, so a clip crosses documents and windows. Text with no header, or past [`MAX_BYTES`],
//!   is no clip and pastes nothing. Refused: a clipboard of the app's own, which no other
//!   window reads.
//! - **A clip that does not fit the document pastes nothing**, and the status bar names why: one
//!   naming a func this library lacks, or a port its declaration lacks, is text from outside, so
//!   the paste lands on a copy of the document first, through the edit gate a widget's edit
//!   takes; a subgraph it carries comes in reconciled, a node it cannot hold kept as a ghost node. Refused: queueing it unchecked, which turns another build's clip into a crash;
//!   dropping the parts that do not fit, which pastes a graph its author never wired.
//! - **A pasted subgraph comes in by [`bring_in`]'s rules**: one equal to the definition the
//!   document holds under its id is that definition, and one that differs comes in under a fresh
//!   id. Cost: a clip holds a constant's path as the document does.

pub(crate) mod error;

use std::collections::BTreeSet;
use std::str;

use common::SerdeFormat;
use glam::Vec2;
use scenarium::{GraphId, GraphSet, Library, Manifest, NodeId, Subgraph, SubgraphId};
use serde::{Deserialize, Serialize};

use crate::core::clip::error::PasteError;
use crate::core::document::Document;
use crate::core::document::graph_view::GraphView;
use crate::core::document::item_id::ItemId;
use crate::core::edit::bring_in::{Carried, bring_in};
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::fragment::Fragment;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::incoming_wires::IncomingWires;
use crate::core::edit::subgraph_intent::{NewSubgraph, SubgraphIntent};

/// The line a clip's text starts with, which names its form.
pub(crate) const HEADER: &str = "darkroom clip 2";

/// How far from where its nodes were copied a paste lands them, so a paste into the graph it was
/// copied from does not hide the original, as a duplicate does.
pub(crate) const PASTE_OFFSET: Vec2 = Vec2::new(32.0, 32.0);

/// The most text a clip is read from: past it the clipboard holds something else.
pub(crate) const MAX_BYTES: usize = 16 << 20;

/// A fragment and each subgraph it places, those placed first, as a copy takes them, with the
/// manifest of the subgraphs: what each func they name declared in the build that copied, so a
/// paste into a build that lacks one keeps a ghost node that says what it was.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Clip {
    fragment: Fragment,
    subgraphs: Vec<ClipSubgraph>,
    manifest: Manifest,
}

/// One subgraph a clip carries, under the id the copied document held it under, with its view.
#[derive(Debug, Serialize, Deserialize)]
struct ClipSubgraph {
    id: SubgraphId,
    subgraph: Subgraph,
    view: GraphView,
}

/// What a paste raises: each subgraph that comes in, then the edits that land the fragment.
#[derive(Debug)]
pub(crate) struct Paste {
    subgraphs: Vec<NewSubgraph>,
    intents: Vec<GraphIntent>,
}

impl Clip {
    /// The graph nodes of `selection` in `graph` of `document`, with each subgraph they place,
    /// however deep; `None` for a selection of no graph node. A wire from outside the selection
    /// stays behind, as it names a node the paste's graph may not hold.
    pub(crate) fn copy(
        document: &Document,
        graph: GraphId,
        library: &Library,
        selection: &BTreeSet<ItemId>,
    ) -> Option<Self> {
        let fragment = Fragment::capture(document, graph, library, selection, IncomingWires::Drop);
        fragment.node_ids().next()?;
        let mut order: Vec<SubgraphId> = Vec::new();
        for id in fragment.placed() {
            for placed in document.graphs.placed_first(id) {
                if !order.contains(&placed) {
                    order.push(placed);
                }
            }
        }
        let subgraphs: Vec<ClipSubgraph> = order
            .into_iter()
            .map(|id| ClipSubgraph {
                id,
                subgraph: document
                    .graphs
                    .subgraph(id)
                    .expect("a copied instance places a held subgraph")
                    .clone(),
                view: document.view(GraphId::Subgraph(id)).clone(),
            })
            .collect();
        let mut carried = GraphSet::default();
        for clip in &subgraphs {
            carried.insert_subgraph(clip.id, clip.subgraph.clone());
        }
        Some(Self {
            fragment,
            manifest: carried.manifest(library),
            subgraphs,
        })
    }

    /// The clip as clipboard text: the header line, then the RON.
    pub(crate) fn to_text(&self) -> String {
        let ron = SerdeFormat::Ron
            .serialize(self)
            .expect("a clip of a document's parts serializes");
        let mut text = String::with_capacity(HEADER.len() + 1 + ron.len());
        text.push_str(HEADER);
        text.push('\n');
        text.push_str(str::from_utf8(&ron).expect("RON is text"));
        text
    }

    /// The clip `text` holds, or `None` for text that is no clip: no header line, past
    /// [`MAX_BYTES`], or RON that is no clip's.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        if text.len() > MAX_BYTES {
            return None;
        }
        let body = text.strip_prefix(HEADER)?.strip_prefix('\n')?;
        SerdeFormat::Ron.deserialize(body.as_bytes()).ok()
    }

    /// Plan pasting this clip into `graph` of `document`, its nodes `offset` from where they were
    /// copied, under fresh ids. Each subgraph it carries comes in by [`bring_in`]'s rules. A clip
    /// that does not fit the document pastes nothing.
    pub(crate) fn paste(
        self,
        document: &Document,
        graph: GraphId,
        library: &Library,
        offset: Vec2,
    ) -> Result<Paste, PasteError> {
        let Self {
            mut fragment,
            subgraphs,
            manifest,
        } = self;
        let carried = subgraphs
            .into_iter()
            .map(|clip| Carried {
                id: clip.id,
                subgraph: clip.subgraph,
                view: clip.view,
                synced: None,
                locked: false,
            })
            .collect();
        let brought = bring_in(document, library, carried, &manifest);
        fragment.repoint(|id| brought.id(id));
        let fragment = fragment.remap(|| NodeId::unique().as_u128());
        let paste = Paste {
            subgraphs: brought.subgraphs,
            intents: fragment.into_intents(offset),
        };
        paste.check(document, graph, library)?;
        Ok(paste)
    }
}

impl Paste {
    /// Land this paste on a copy of `document`: a clip is text from outside, so an edit the gate
    /// takes as malformed is the clip's fault, which the paste refuses, not a widget's bug, which
    /// panics. The subgraphs it brings come in reconciled ([`bring_in`]). A copy of the document
    /// per paste, a click's cost.
    fn check(
        &self,
        document: &Document,
        graph: GraphId,
        library: &Library,
    ) -> Result<(), PasteError> {
        let mut trial = document.clone();
        let mut steps = Vec::new();
        for subgraph in &self.subgraphs {
            SubgraphIntent::Add(Box::new(subgraph.clone()))
                .commit(&mut trial, library, &mut steps)
                .map_err(PasteError)?;
        }
        for intent in &self.intents {
            intent
                .clone()
                .commit(&mut trial, graph, library, &mut steps)
                .map_err(PasteError)?;
        }
        debug_assert!(
            trial.graphs.validate_with(library).is_ok(),
            "a paste of gate-checked edits and reconciled definitions holds the load contract: {:?}",
            trial.graphs.validate_with(library)
        );
        Ok(())
    }

    /// Queue this paste into `graph`, in the order it lands: one batch, so one entry.
    pub(crate) fn queue(self, graph: GraphId, queue: &mut DocumentQueue) {
        for subgraph in self.subgraphs {
            queue.push_subgraph(SubgraphIntent::Add(Box::new(subgraph)));
        }
        queue.extend_graph(graph, self.intents);
    }
}

#[cfg(test)]
mod tests;
