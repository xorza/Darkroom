//! Structural validation for documents and their per-graph editor views.

use anyhow::{Context, Result, ensure};
use common::is_debug;
use scenarium::Graph as CoreGraph;

use crate::core::document::{Document, GraphView, ItemRef, tab_alive};

impl GraphView {
    fn check(&self, graph: &CoreGraph) -> Result<()> {
        ensure!(
            self.viewport.is_valid(),
            "graph viewport must have finite pan and positive finite zoom"
        );

        // IndexMap guarantees unique keys, so counts plus reverse membership
        // prove the graph and view contain exactly the same node and pin sets.
        let mut node_items = 0usize;
        for (key, position) in &self.item_placements {
            ensure!(
                position.is_finite(),
                "view item {:?} position must be finite",
                key
            );
            match key {
                ItemRef::Node(_) => node_items += 1,
                ItemRef::Pin(port) => ensure!(
                    graph.is_output_pinned(*port),
                    "view item references an output that isn't pinned"
                ),
            }
        }
        ensure!(
            node_items == graph.len(),
            "view node items must match graph nodes"
        );
        for node in graph.iter() {
            ensure!(
                self.item_placements.get(&ItemRef::Node(node.id)).is_some(),
                "graph view missing a position for node {:?}",
                node.id
            );
        }
        for port in graph.pinned_outputs() {
            ensure!(
                self.item_placements.get(&ItemRef::Pin(port)).is_some(),
                "pinned output must have a view item"
            );
        }
        for key in &self.selected {
            ensure!(
                self.item_placements.get(key).is_some(),
                "selected item {key:?} has no view item"
            );
        }
        Ok(())
    }
}

impl Document {
    /// Full structural validation for untrusted documents.
    pub(crate) fn check(&self) -> Result<()> {
        self.graph.check()?;
        ensure!(
            self.graph.inputs.is_empty()
                && self.graph.outputs.is_empty()
                && self.graph.events.is_empty(),
            "entry graph cannot expose an interface"
        );
        ensure!(
            self.graph.iter().all(|node| !node.kind.is_boundary()),
            "entry graph cannot contain interface boundary nodes"
        );

        self.main_view.check(&self.graph).context("main view")?;
        for (id, view) in &self.local_views {
            let graph = self.graph.graphs.get(id).with_context(|| {
                format!("local_views entry references missing local graph {id:?}")
            })?;
            view.check(graph)
                .with_context(|| format!("graph {id:?} view"))?;
        }

        self.layout.check()?;
        for tab in self.layout.all_tabs() {
            ensure!(
                tab_alive(&self.graph, tab),
                "open tab references a missing target {tab:?}"
            );
        }
        Ok(())
    }

    /// Debug-only assert form of [`Self::check`].
    pub(crate) fn debug_check(&self) {
        if !is_debug() {
            return;
        }
        self.check()
            .expect("document structural invariant violated");
    }
}
