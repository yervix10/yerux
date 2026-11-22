# `std.compress`

`compress.compress(String) -> String` and `compress.decompress(String) ->
Result<String, String>` implement a byte-oriented RLE format: each pair is a
run length (1–255) followed by its byte. It supports arbitrary byte content
stored in `String`, but provides no framing, checksum, or gzip/deflate
compatibility. Decompression rejects odd-length input and zero-length runs.
