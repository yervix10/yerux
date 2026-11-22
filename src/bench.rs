// `yerux bench`: runs the `bench "name" { .. }` blocks of .yx files and reports wall times.
//
// A bench block is compiled like a `test` block (a program whose `main` is the block body). The runner executes it
// once as a warm-up (that run also decides pass/fail: a non-zero exit is a failure) and then `--runs N` timed runs
// (default 5), and prints min / median / mean of the wall time of the whole process.

use crate::{build_block_exe, modules};
use std::{fs, path::Path, process::Command, time::Instant};

fn collect(dir: &Path, out: &mut Vec<String>) {
    let mut entries: Vec<_> = match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
        Err(_) => return,
    };
    entries.sort();
    for p in entries {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if p.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                collect(&p, out);
            }
        } else if name.ends_with(".yx")
            && fs::read_to_string(&p).is_ok_and(|s| s.contains("bench \""))
        {
            out.push(p.to_string_lossy().trim_start_matches("./").to_string());
        }
    }
}

fn human(ns: u128) -> String {
    if ns >= 1_000_000_000 {
        format!("{:.2} s", ns as f64 / 1e9)
    } else if ns >= 1_000_000 {
        format!("{:.2} ms", ns as f64 / 1e6)
    } else if ns >= 1_000 {
        format!("{:.2} us", ns as f64 / 1e3)
    } else {
        format!("{} ns", ns)
    }
}

/// Summary of a list of timings (ns): (min, median, mean).
pub fn summarize(times: &[u128]) -> (u128, u128, u128) {
    let mut t = times.to_vec();
    t.sort();
    let n = t.len().max(1);
    (
        t.first().copied().unwrap_or(0),
        t.get(n / 2).copied().unwrap_or(0),
        t.iter().sum::<u128>() / n as u128,
    )
}

pub fn run_cli(args: &[String]) -> i32 {
    let (mut native, mut optimize, mut filter, mut runs) = (false, true, None::<String>, 5usize);
    let mut paths: Vec<String> = vec![];
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--native" => native = true,
            "--no-opt" => optimize = false,
            "--filter" => {
                i += 1;
                filter = args.get(i).cloned();
            }
            "--runs" => {
                i += 1;
                match args.get(i).and_then(|n| n.parse::<usize>().ok()) {
                    Some(n) if n >= 1 => runs = n,
                    _ => {
                        eprintln!("--runs needs a number >= 1");
                        return 2;
                    }
                }
            }
            a if a.starts_with("--") => {
                eprintln!("unknown option {}\nusage: yerux bench [file.yx|dir ...] [--native] [--no-opt] [--filter text] [--runs N]", a);
                return 2;
            }
            a => paths.push(a.to_string()),
        }
        i += 1;
    }
    if paths.is_empty() {
        // inside a project (yerux.toml) the benchmarks live in src/ and tests/
        if Path::new("yerux.toml").is_file() {
            paths.extend(
                ["src", "tests"]
                    .iter()
                    .filter(|d| Path::new(d).is_dir())
                    .map(|d| d.to_string()),
            );
        } else {
            paths.push(".".into());
        }
    }
    let mut files: Vec<String> = vec![];
    for p in &paths {
        let pa = Path::new(p);
        if pa.is_dir() {
            collect(pa, &mut files);
        } else if pa.is_file() {
            files.push(p.clone());
        } else {
            eprintln!("could not read {}", p);
            return 2;
        }
    }
    let (mut done, mut failed) = (0usize, 0usize);
    for f in &files {
        let names = match modules::load(f, modules::Mode::Normal) {
            Ok(l) => l.benches,
            Err(msgs) => {
                for m in &msgs {
                    eprintln!("{}", m);
                }
                failed += 1;
                continue;
            }
        };
        let wanted: Vec<(usize, &String)> = names
            .iter()
            .enumerate()
            .filter(|(_, n)| filter.as_ref().is_none_or(|x| n.contains(x.as_str())))
            .collect();
        if wanted.is_empty() {
            continue;
        }
        println!("{}", f);
        for (idx, name) in wanted {
            let exe = match build_block_exe(f, modules::Mode::Bench(idx), native, optimize) {
                Ok(e) => e,
                Err(msgs) => {
                    failed += 1;
                    println!("  FAIL  {}  (does not compile)", name);
                    for m in &msgs {
                        for l in m.lines() {
                            println!("        {}", l);
                        }
                    }
                    continue;
                }
            };
            let mut times: Vec<u128> = vec![];
            let mut bad = None;
            for run in 0..=runs {
                let t0 = Instant::now();
                let st = Command::new(&exe)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                let ns = t0.elapsed().as_nanos();
                match st {
                    Ok(s) if s.success() => {
                        if run > 0 {
                            times.push(ns);
                        }
                    }
                    Ok(s) => {
                        bad = Some(format!("exit status {}", s.code().unwrap_or(-1)));
                        break;
                    }
                    Err(e) => {
                        bad = Some(e.to_string());
                        break;
                    }
                }
            }
            let _ = fs::remove_file(&exe);
            if let Some(why) = bad {
                failed += 1;
                println!("  FAIL  {}  ({})", name, why);
                continue;
            }
            done += 1;
            let (min, med, mean) = summarize(&times);
            println!(
                "  bench {}  min {}  median {}  mean {}  ({} runs)",
                name,
                human(min),
                human(med),
                human(mean),
                times.len()
            );
        }
    }
    if done + failed == 0 {
        println!("no benchmarks found (a benchmark is `bench \"name\" {{ ... }}` at the top level of a .yx file)");
        return 0;
    }
    println!();
    println!(
        "bench result: {}. {} measured; {} failed",
        if failed == 0 { "ok" } else { "FAILED" },
        done,
        failed
    );
    if failed == 0 {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_is_min_median_mean() {
        assert_eq!(summarize(&[30, 10, 20]), (10, 20, 20));
        assert_eq!(summarize(&[5]), (5, 5, 5));
        assert_eq!(summarize(&[]), (0, 0, 0));
    }

    #[test]
    fn human_units() {
        assert_eq!(human(500), "500 ns");
        assert_eq!(human(2_500), "2.50 us");
        assert_eq!(human(3_000_000), "3.00 ms");
        assert_eq!(human(2_000_000_000), "2.00 s");
    }
}
