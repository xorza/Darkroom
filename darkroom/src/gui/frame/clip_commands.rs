//! Copy and Paste between the active graph and the system clipboard, as the canvas's chords and
//! the Edit menu raise them.

use std::collections::BTreeSet;

use common::ErrorChain;
use palantir::Ui;
use scenarium::{GraphId, Library};

use crate::core::clip::{Clip, PASTE_OFFSET};
use crate::core::document::Document;
use crate::core::document::item_id::ItemId;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::requests::Requests;

/// Put the graph nodes of `selection` in `graph` on the clipboard as a clip; nothing for a
/// selection of no graph node.
pub(crate) fn copy(
    ui: &Ui,
    document: &Document,
    graph: GraphId,
    library: &Library,
    selection: &BTreeSet<ItemId>,
) {
    if let Some(clip) = Clip::copy(document, graph, library, selection) {
        ui.clipboard().set_text(&clip.to_text());
    }
}

/// Queue a paste of the clipboard's clip into `graph`; nothing when the clipboard holds no clip
/// or cannot be read, and a warning naming why for a clip that does not fit the document.
pub(crate) fn paste(
    ui: &Ui,
    document: &Document,
    graph: GraphId,
    library: &Library,
    out: &mut Requests,
) {
    let Ok(Some(text)) = ui.clipboard().text() else {
        return;
    };
    let Some(clip) = Clip::parse(&text) else {
        return;
    };
    match clip.paste(document, graph, library, PASTE_OFFSET) {
        Ok(paste) => paste.queue(graph, out.document()),
        Err(error) => out.push_app(AppCommand::Warn(ErrorChain::new(&error).to_string())),
    }
}
