# `std.kv`

`import std.kv` provides `kv.open(path) -> Result<Store, String>`, with
`kv.set/get` for strings and `kv.set_int/get_int` and `kv.set_bool/get_bool`
for typed scalar values. Reads return `Result<Option<T>, String>`; a missing
key is `Ok(None)`. The file stores hex-encoded UTF-8 keys and values, one pair
per line. Updates write `path.tmp` and rename it over the database, so readers
see a complete old or new file. This is a small single-writer store: it does
not coordinate concurrent processes or compact repeated updates.
