// The embedded standard-library modules: `import std.io` loads src/std/io.yx from inside the compiler, so no
// file has to exist next to the program. Everything else about a std module is an ordinary module (modules.rs).

/// Source text of the standard-library module `key` (`std.io`), if there is one.
pub fn source(key: &str) -> Option<&'static str> {
    match key {
        "std.io" => Some(include_str!("std/io.yx")),
        "std.text" => Some(include_str!("std/text.yx")),
        "std.sort" => Some(include_str!("std/sort.yx")),
        "std.iter" => Some(include_str!("std/iter.yx")),
        "std.fmt" => Some(include_str!("std/fmt.yx")),
        "std.time" => Some(include_str!("std/time.yx")),
        "std.path" => Some(include_str!("std/path.yx")),
        "std.math" => Some(include_str!("std/math.yx")),
        "std.fs" => Some(include_str!("std/fs.yx")),
        "std.env" => Some(include_str!("std/env.yx")),
        "std.net" => Some(include_str!("std/net.yx")),
        _ => None,
    }
}
