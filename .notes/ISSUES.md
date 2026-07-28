# Issues noticed in passing

- `palantir/src/widgets/popup/tests.rs:614` — the doc comment on
  `text_edit_inside_a_popup_receives_typing` describes a
  `with_keyboard_claim` call and a whole-body keyboard capture that the
  input-scope rewrite removed; `Popup::show` now declares a `KeyFilter::ALL`
  scope instead.
