# Issues

- `value_editor::read_only_label` builds a fresh `String` from the literal on
  every frame it records (`ConstValue::to_value_string` into a throwaway
  `TextEdit` buffer), for every port whose stored literal falls outside its
  declared type's coercion class.

- `value_editor::show` collects a `Vec<&str>` of option names on every frame
  for each port that renders a dropdown — once for a port carrying
  `value_variants`, once for an `Enum` port's registered variants.
