use super::*;
use crate::gui::theme::Theme;
use glam::UVec2;
use palantir::internals::ui_harness::UiHarness;
use palantir::{Display, Key};

/// The idle label puts the name flush against its panel's leading edge,
/// where the editor will draw it. Pixel snapping off keeps the comparison
/// exact.
#[test]
fn the_idle_name_sits_on_the_panels_leading_edge() {
    let theme = Theme::default();
    let wid = WidgetId::from_hash("rename-leading-edge");
    let surface = UVec2::new(300, 100);
    let mut h = UiHarness::new(surface);
    h.set_display(Display {
        physical: surface,
        pixel_snap: false,
        ..Display::default()
    });
    h.frame(|ui| {
        InlineRename::new(wid, "Ab", &theme.flat_text_edit.plain).show(ui);
    });
    let panel = h.rect(wid).expect("label panel arranged");
    let label = h.rect(label_wid(wid)).expect("name text arranged");
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
    fn render(ui: &mut Ui, wid: WidgetId, theme: &Theme) -> RenameEvent {
        InlineRename::new(wid, "Alpha", &theme.flat_text_edit.plain).show(ui)
    }

    let theme = Theme::default();
    let wid = WidgetId::from_hash("rename-select-all");
    let mut h = UiHarness::new(UVec2::new(300, 100));

    // Lay the label out, then double-click it to open the editor.
    h.frame(|ui| {
        render(ui, wid, &theme);
    });
    let hit = h.rect(wid).expect("label arranged").center();
    h.click_at(hit);
    h.frame(|ui| {
        render(ui, wid, &theme);
    });
    h.click_at(hit);
    h.frame(|ui| {
        render(ui, wid, &theme);
    });

    // The editor's first frame: focus lands and the draft selects.
    h.frame(|ui| {
        render(ui, wid, &theme);
    });

    // One character replaces the selection outright; the next appends,
    // which only holds if the draft survived the frame between them —
    // `show` hands its buffer back to the state row every frame rather
    // than copying it out.
    h.key(Key::Char('X'));
    h.frame(|ui| {
        render(ui, wid, &theme);
    });
    h.key(Key::Char('Y'));
    h.frame(|ui| {
        render(ui, wid, &theme);
    });

    h.key(Key::Enter);
    let committed = h.frame_value(|ui| render(ui, wid, &theme));
    assert_eq!(
        committed,
        RenameEvent::Committed("XY".to_string()),
        "the first keystroke must replace the whole name, not splice into \
         it — and the draft must carry across frames from there",
    );

    // A read-only label opens no editor, so the same gesture commits nothing.
    let locked = WidgetId::from_hash("rename-read-only");
    let render_locked = |ui: &mut Ui| {
        InlineRename::new(locked, "Alpha", &theme.flat_text_edit.plain)
            .read_only()
            .show(ui)
    };
    h.frame(|ui| {
        render_locked(ui);
    });
    let hit = h.rect(locked).expect("label arranged").center();
    for _ in 0..2 {
        h.click_at(hit);
        h.frame(|ui| {
            render_locked(ui);
        });
    }
    h.key(Key::Char('X'));
    h.key(Key::Enter);
    let committed = h.frame_value(|ui| render_locked(ui));
    assert_eq!(
        committed,
        RenameEvent::Idle,
        "a read-only name never commits"
    );
    assert!(
        h.rect(label_wid(locked)).is_some(),
        "the label stays a label"
    );
}
