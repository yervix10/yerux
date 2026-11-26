# `std.unicode`

Unicode scalar helpers for broad letter/decimal/whitespace checks, approximate terminal cell widths, and a small Latin NFC composition subset. `is_letter` and `is_number` use curated ranges rather than the complete Unicode Character Database; normalization is intentionally limited. Tests: `tests/unicode_basic.yx`, `tests/std_unicode_combining.yx`.

| Function | Result |
|---|---|
| `is_letter(c: Char) -> Bool` | letters in curated ranges |
| `is_number(c: Char) -> Bool` | decimal digits of common scripts |
| `is_whitespace(c: Char) -> Bool` | Unicode white space used by text formats |
| `is_combining(c: Char) -> Bool` | common combining-mark blocks |
| `char_width(c: Char) -> Int` | approximate terminal cells: 0 for controls and combining marks, 2 for wide CJK and emoji, else 1 |
| `width(s: view String) -> Int` | sum of `char_width` over the characters |
| `normalize_nfc(s: view String) -> String` | composes the supported Latin letter + combining mark pairs; other text is unchanged |
