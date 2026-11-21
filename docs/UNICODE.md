# `std.unicode`

Unicode scalar helpers for broad letter/decimal/whitespace checks, approximate terminal cell widths, and a small Latin NFC composition subset. `is_letter` and `is_number` use curated ranges rather than the complete Unicode Character Database; normalization is intentionally limited. Test: `tests/unicode_basic.yx`.
