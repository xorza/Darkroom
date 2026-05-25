//! Inline rename editor for a subgraph boundary port's name. Renders a
//! plain label that swaps to a fixed-width `TextEdit` on double-click;
//! Enter / focus-loss commits an [`Intent::RenameBoundaryPort`], Esc
//! cancels. Used only by the boundary (`SubgraphInput`/`SubgraphOutput`)
//! port rows in [`crate::gui::node::port_row`]; ordinary node ports render
//! plain text. Mirrors the per-widget split of [`crate::gui::node::value_editor`].

use palantir::{
    Configure, InternedStr, Key, Panel, Sense, Shortcut, Sizing, Spacing, Stroke, Text, TextEdit,
    TextEditTheme, Ui, WidgetId,
};

use crate::document::BoundarySide;
use crate::gui::PortRef;
use crate::gui::node::{RecordCtx, select_intent};
use crate::intent::Intent;

/// Character cap for a boundary-port name in the inline rename editor.
const PORT_NAME_MAX_CHARS: usize = 24;

/// Cross-frame state for a boundary port's inline rename editor, held in
/// palantir's `StateMap` under the editor's `WidgetId`.
#[derive(Default, Clone)]
struct PortRename {
    active: bool,
    /// Latches once the editor actually holds focus, so the frames
    /// between `request_focus` and focus landing don't read as a blur
    /// and commit early.
    focused_once: bool,
    draft: String,
}

/// Stable id for a port's rename editor — and for the sensing label
/// panel shown when idle, so the same `WidgetId` is recorded every frame
/// across the label⇄editor swap (palantir drops state rows for ids it
/// doesn't see).
fn port_rename_wid(port: PortRef) -> WidgetId {
    WidgetId::from_hash((
        "graph.node.port_rename",
        port.node_id,
        port.kind as u8,
        port.port_idx,
    ))
}

/// A port label. When `rename` is `Some`, double-clicking swaps the
/// label for a fixed-width, length-capped `TextEdit`; Enter or focus
/// loss commits a [`Intent::RenameBoundaryPort`], Esc cancels. `None`
/// (regular node ports and the trailing "+" placeholder) renders plain
/// text.
pub(super) fn port_label(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    port: PortRef,
    name: InternedStr,
    rename: Option<BoundarySide>,
    out: &mut Vec<Intent>,
) {
    let theme = rcx.theme;
    let Some(side) = rename else {
        Text::new(name).show(ui);
        return;
    };
    let id = port_rename_wid(port);
    // darkroom port names are always `Owned` (built via `String::into`),
    // so `as_str` resolves without a live text arena. Resolve lazily —
    // only the double-click seed and the commit compare need it, never
    // the idle per-frame path.
    if !ui.state_mut::<PortRename>(id).active {
        let shift = ui.modifiers().shift;
        let resp = Panel::hstack()
            .id(id)
            .size((Sizing::Hug, Sizing::Hug))
            .sense(Sense::CLICK)
            .show(ui, |ui| {
                // `InternedStr::clone` is allocation-free for the `Owned`
                // names darkroom builds, so this is cheap per frame.
                Text::new(name.clone()).show(ui);
            })
            .response;
        // Single click selects the node (the label otherwise swallows
        // the click the body would have gotten); double-click renames.
        if resp.clicked() {
            out.push(select_intent(shift, rcx.scene, port.node_id));
        }
        if resp.double_clicked() {
            let st = ui.state_mut::<PortRename>(id);
            st.active = true;
            st.focused_once = false;
            st.draft = name.as_str("").to_owned();
            ui.request_focus(Some(id));
        }
        return;
    }

    let mut draft = std::mem::take(&mut ui.state_mut::<PortRename>(id).draft);
    TextEdit::new(&mut draft)
        .id(id)
        .style(flat_edit_style(ui))
        .max_chars(PORT_NAME_MAX_CHARS)
        .size((Sizing::Fixed(theme.value_editor_width), Sizing::Hug))
        .show(ui);
    let focused = ui.focused_id() == Some(id);
    let escape = ui.escape_pressed();
    let enter = ui.key_pressed(Shortcut::key(Key::Enter));
    let commit = {
        let st = ui.state_mut::<PortRename>(id);
        st.draft = draft.clone();
        st.focused_once |= focused;
        // Commit on Enter or on blur (once focus had landed); Esc wins
        // as a cancel.
        !escape && (enter || (st.focused_once && !focused))
    };
    if commit || escape {
        if commit && draft != name.as_str("") {
            out.push(Intent::RenameBoundaryPort {
                side,
                idx: port.port_idx,
                to: draft,
            });
        }
        let st = ui.state_mut::<PortRename>(id);
        st.active = false;
        st.focused_once = false;
        ui.request_focus(None);
    }
}

/// The ambient text-edit theme flattened for an inline port-rename
/// field: zero padding/margin and no border, so the editor's `Hug`
/// height equals the plain `Text` label's line height — the node body
/// doesn't grow when a label enters/exits edit mode. The fill stays, so
/// the fixed-width field still reads as editable.
fn flat_edit_style(ui: &Ui) -> TextEditTheme {
    let mut style = ui.theme.text_edit.clone();
    style.padding = Spacing::ZERO;
    style.margin = Spacing::ZERO;
    for look in [&mut style.normal, &mut style.focused, &mut style.disabled] {
        if let Some(bg) = look.background.as_mut() {
            bg.stroke = Stroke::ZERO;
        }
    }
    style
}
