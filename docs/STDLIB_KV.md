# `std.kv`

`import std.kv` provides `kv.open(path) -> Result<Store, String>`, with
`kv.set/get` for strings and `kv.set_int/get_int` and `kv.set_bool/get_bool`
for typed scalar values. Reads return `Result<Option<T>, String>`; a missing
key is `Ok(None)`. The file stores hex-encoded UTF-8 keys and values, one pair
per line. Updates write `path.tmp` and rename it over the database, so readers
see a complete old or new file. This is a small single-writer store: it does
not coordinate concurrent processes or compact repeated updates.

`kv.delete(store, key) -> Result<Bool, String>` removes a key by appending a tombstone record (`keyhex<TAB>-`); the result
is whether the key existed, and a later `get` reports it missing. `kv.keys(store) -> Result<List<String>, String>`
lists the live keys in the order they were first written. Like `set`, both rewrite the whole file through
`path.tmp`. (Added while writing `examples/real/minidb.yx`, which keeps its saved tables in a store.)
