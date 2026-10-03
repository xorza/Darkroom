use super::*;
use crate::gui::theme::Theme;
use palantir::Display;
use palantir::internals::harness::UiHarness;

/// The idle label puts the name flush against its panel's leading edge,
/// where the editor will draw it. Pixel snapping off keeps the comparison
/// exact.
#[test]
fn the_idle_name_sits_on_the_panels_leading_edge() {
    let theme = Theme::default();
    let id = WidgetId::from_hash("rename-leading-edge");
    let surface = UVec2::new(300, 100);
    let mut h = UiHarness::new(surface);
    h.set_display(Display {
        pixel_snap: false,
        ..Display::from_physical(surface, 1.0)
    });
    h.frame(|ui| {
        InlineRename::new("Ab", &theme.inline_rename)
            .id(id)
            .show(ui);
    });
    let panel = h.rect(id).expect("label panel arranged");
    let label = h.rect(label_wid(id)).expect("name text arranged");
    assert_eq!(label.min.x, panel.min.x);
}

/// Entering rename selects the whole draft, so the first keystroke
/// replaces the name instead of appending to it.
///
/// Asserted through the committed value rather than the editor's
/// selection range — that's the behaviour the caller sees, and the
/// range is palantir-internal anyway. Without `select_all_on_focus`
/// the caret sits where the double-click landed and this commits
/// some splice of the old name and the new character.
#[test]
fn entering_edit_mode_selects_the_whole_name() {
    fn render(ui: &mut Ui, id: WidgetId, theme: &Theme) -> RenameEvent {
        InlineRename::new("Alpha", &theme.inline_rename)
            .id(id)
            .show(ui)
    }

    let theme = Theme::default();
    let id = WidgetId::from_hash("rename-select-all");
    let mut h = UiHarness::new(UVec2::new(300, 100));

    // Lay the label out, then double-click it to open the editor.
    h.frame(|ui| {
        render(ui, id, &theme);
    });
    let hit = h.rect(id).expect("label arranged").center();
    h.click_at(hit);
    h.frame(|ui| {
        render(ui, id, &theme);
    });
    h.click_at(hit);
    h.frame(|ui| {
        render(ui, id, &theme);
    });

    // The editor's first frame: focus lands and the draft selects.
    h.frame(|ui| {
        render(ui, id, &theme);
    });

    // One character replaces the selection outright; the next appends,
    // which only holds if the draft survived the frame between them —
    // `show` hands its buffer back to the state row every frame rather
    // than copying it out.
    h.key(Key::Char('X'));
    h.frame(|ui| {
        render(ui, id, &theme);
    });
    h.key(Key::Char('Y'));
    h.frame(|ui| {
        render(ui, id, &theme);
    });

    h.key(Key::Enter);
    let committed = h.frame_value(|ui| render(ui, id, &theme).committed);
    assert_eq!(
        committed.as_deref(),
        Some("XY"),
        "the first keystroke must replace the whole name, not splice into \
         it — and the draft must carry across frames from there",
    );
}
