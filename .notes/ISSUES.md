# Issues

- `Palette::badge_impure`'s doc says it shares the palette's one bright
  magenta with `TypeColors::image`. The asset gives `badge_impure`
  `#ebebeb` and `image` `#f8a45c`.

- `gui::theme::tests::default_wiring_and_menu_tweak` asserts
  `theme.card.min_width == 160.0`, a literal value the same test's doc
  says it asserts against the palette instead of.
