// The embedded standard-library modules: `import std.io` loads src/std/io.yx from inside the compiler, so no
// file has to exist next to the program. Everything else about a std module is an ordinary module (modules.rs).

/// Every embedded standard-library module.
pub const MODULES: &[&str] = &["std.io", "std.text", "std.sort", "std.iter", "std.fmt", "std.args", "std.collections", "std.time", "std.path", "std.math", "std.fs", "std.env", "std.net", "std.process", "std.hex", "std.hash", "std.rand", "std.log", "std.http", "std.http.server", "std.unicode", "std.compress", "std.kv", "std.sync", "std.caps"];

/// Source text of the standard-library module `key` (`std.io`), if there is one.
pub fn source(key: &str) -> Option<&'static str> {
    match key {
        "std.io" => Some(include_str!("std/io.yx")),
        "std.text" => Some(include_str!("std/text.yx")),
        "std.sort" => Some(include_str!("std/sort.yx")),
        "std.iter" => Some(include_str!("std/iter.yx")),
        "std.fmt" => Some(include_str!("std/fmt.yx")),
        "std.args" => Some(include_str!("std/args.yx")),
        "std.collections" => Some(include_str!("std/collections.yx")),
        "std.time" => Some(include_str!("std/time.yx")),
        "std.path" => Some(include_str!("std/path.yx")),
        "std.math" => Some(include_str!("std/math.yx")),
        "std.fs" => Some(include_str!("std/fs.yx")),
        "std.env" => Some(include_str!("std/env.yx")),
        "std.net" => Some(include_str!("std/net.yx")),
        "std.process" => Some(include_str!("std/process.yx")),
        "std.hex" => Some(include_str!("std/hex.yx")),
        "std.base64" => Some(include_str!("std/base64.yx")),
        "std.hash" => Some(include_str!("std/hash.yx")),
        "std.rand" => Some(include_str!("std/rand.yx")),
        "std.log" => Some(include_str!("std/log.yx")),
        "std.http" => Some(include_str!("std/http.yx")),
        "std.http.server" => Some(include_str!("std/http_server.yx")),
        "std.unicode" => Some(include_str!("std/unicode.yx")),
        "std.compress" => Some(include_str!("std/compress.yx")),
        "std.kv" => Some(include_str!("std/kv.yx")),
        "std.sync" => Some(include_str!("std/sync.yx")),
        "std.caps" => Some(include_str!("std/caps.yx")),
        "std.regex" => Some(include_str!("std/regex.yx")),
        "std.csv" => Some(include_str!("std/csv.yx")),
        "std.toml" => Some(include_str!("std/toml.yx")),
        _ => None,
    }
}
