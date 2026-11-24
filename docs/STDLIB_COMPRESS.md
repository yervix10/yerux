# `std.compress`

`std.compress` is the standard library's byte-oriented run-length encoding
(RLE) module. The wire format is a sequence of `(count, byte)` pairs, with
one byte per field. The encoder splits runs longer than 255 into consecutive
pairs. The empty input encodes as an empty output. The format has no framing
or checksum. Gzip and deflate use different formats.

- `compress.compress(input: view String) -> String` encodes the bytes of the
  input. Both input and output can contain arbitrary bytes, including NUL.
- `compress.decompress(input: view String) -> Result<String, String>` returns
  an error for odd-length inputs, counts of zero or decoded output greater
  than 16 MiB.

The exact wire format is covered by `tests/std_compress_format.yx` and the
decoding limit by `tests/std_compress_hostile.yx`.
