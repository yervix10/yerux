#!/usr/bin/env python3
"""Yerux benchmark suite driver.

Builds every benchmark with every toolchain, checks that all outputs are identical,
runs each binary N times (interleaved), and writes RESULTS.md.

  python3 bench/run.py [--runs 7] [--only fib,sieve] [--skip-build] [--no-write]

Timing is CPU time (user+sys) taken from wait4() rusage, peak RSS is ru_maxrss.
"""
import argparse, math, os, shutil, statistics, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SRC = os.path.join(HERE, "src")
BUILD = os.path.join(HERE, "build")
YERUX = os.path.join(ROOT, "target", "release", "yerux")
RUSTC = os.environ.get("RUSTC", "/root/.cargo/bin/rustc")
GO = os.environ.get("GO", "/usr/local/go/bin/go")
BENCHES = ["arith", "fib", "sieve", "sort", "strbuild", "structs", "alloc", "matmul", "nbody", "mandelbrot", "dot"]
DESC = {
    "arith": "100M-iteration checked arithmetic loop",
    "fib": "fib(40) recursion (spec asked 38; raised so the run is not ~0.1 s)",
    "sieve": "sieve of Eratosthenes, List<Bool> of 10M",
    "sort": "bottom-up merge sort, 200k List<Int>, 10 rounds",
    "strbuild": "build 8M x (push_str + push_int) = 71 MB string, then byte scan",
    "structs": "1000 Copy-struct particles x 100000 steps (get/step/set by value)",
    "alloc": "6M x (List<Int> of 8 + String) create/drop",
    "matmul": "300x300 flat List<Int> matmul, 5 rounds",
    "nbody": "5-body n-body (List<Float> state, sqrt), 2M steps",
    "mandelbrot": "800x600 Mandelbrot, 200 iterations (Float locals)",
    "dot": "dot product of two 1M List<Float>, 200 rounds",
}
# key -> (label, builder). builder(bench, out) -> argv list
LANGS = [
    ("c", "C gcc -O2", lambda b, o: ["gcc", "-O2", "-o", o, f"{SRC}/c/{b}.c"]),
    ("cc", "C gcc -O2 + checks", lambda b, o: ["gcc", "-O2", "-DCHECKED", "-o", o, f"{SRC}/c/{b}.c"]),
    ("rs", "Rust opt3 (release)", lambda b, o: [RUSTC, "-C", "opt-level=3", "-o", o, f"{SRC}/rs/{b}.rs"]),
    ("rc", "Rust opt3 + overflow-checks", lambda b, o: [RUSTC, "-C", "opt-level=3", "-C", "overflow-checks=on", "-o", o, f"{SRC}/rs/{b}.rs"]),
    ("go", "Go", lambda b, o: [GO, "build", "-o", o, f"{SRC}/go/{b}/main.go"]),
    ("yn", "Yerux native", lambda b, o: [YERUX, "build", f"{SRC}/{b}.yx", "--native", "-o", o]),
    ("yc", "Yerux C backend", lambda b, o: [YERUX, "build", f"{SRC}/{b}.yx", "-o", o]),
]
KEYS = [k for k, _, _ in LANGS]
LABEL = {k: l for k, l, _ in LANGS}


def build(benches):
    shutil.rmtree(BUILD, ignore_errors=True)
    os.makedirs(BUILD)
    ctime, size = {}, {}
    env = dict(os.environ, GOCACHE=os.path.join(BUILD, "gocache"), GOFLAGS="-trimpath")
    for b in benches:
        for k, _, mk in LANGS:
            out = os.path.join(BUILD, f"{k}_{b}")
            t = time.perf_counter()
            r = subprocess.run(mk(b, out), capture_output=True, text=True, env=env)
            ctime[(b, k)] = time.perf_counter() - t
            if r.returncode != 0:
                sys.exit(f"build failed: {k} {b}\n{r.stderr}")
            size[(b, k)] = os.path.getsize(out)
    return ctime, size


def run_once(path):
    r, w = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.dup2(w, 1); os.close(r); os.close(w)
        os.execv(path, [path])
    os.close(w)
    chunks = []
    while True:
        c = os.read(r, 65536)
        if not c: break
        chunks.append(c)
    os.close(r)
    _, status, ru = os.wait4(pid, 0)
    return b"".join(chunks).decode(), os.waitstatus_to_exitcode(status), ru.ru_utime + ru.ru_stime, ru.ru_maxrss


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=7)
    ap.add_argument("--only", default="")
    ap.add_argument("--skip-build", action="store_true")
    ap.add_argument("--no-write", action="store_true")
    a = ap.parse_args()
    benches = a.only.split(",") if a.only else BENCHES
    if a.skip_build:
        ctime, size = {}, {}
        for b in benches:
            for k in KEYS:
                size[(b, k)] = os.path.getsize(os.path.join(BUILD, f"{k}_{b}"))
    else:
        ctime, size = build(benches)

    # correctness: all outputs identical to plain C
    expected = {}
    for b in benches:
        outs = {}
        for k in KEYS:
            o, rc, _, _ = run_once(os.path.join(BUILD, f"{k}_{b}"))
            if rc != 0: sys.exit(f"{k} {b} exited {rc}")
            outs[k] = o
        expected[b] = outs["c"]
        bad = [k for k in KEYS if outs[k] != outs["c"]]
        if bad: sys.exit(f"OUTPUT MISMATCH in {b}: {bad}\n{outs}")
        print(f"verified {b}: {outs['c'].split()}")

    times = {(b, k): [] for b in benches for k in KEYS}
    rss = {(b, k): [] for b in benches for k in KEYS}
    for i in range(a.runs):
        for b in benches:
            for k in KEYS:
                o, rc, t, m = run_once(os.path.join(BUILD, f"{k}_{b}"))
                assert o == expected[b] and rc == 0
                times[(b, k)].append(t); rss[(b, k)].append(m)
        print(f"run {i+1}/{a.runs} done", flush=True)

    md = ["# Yerux benchmark results", "",
          f"Generated by `bench/run.py` ({a.runs} runs per binary, interleaved). Machine: "
          f"{os.cpu_count()} CPUs, `{subprocess.getoutput('uname -sr')}`. All outputs verified identical to the C version.", "",
          "CPU time = user+sys from `wait4`. `best`/`median` in seconds; `spread` = (max-min)/median over the runs (noise indicator); "
          "ratios are relative to plain `C gcc -O2` (lower is better; ratio of best / ratio of median). RSS = peak resident set (MB).", ""]
    ratios = {k: [] for k in KEYS}
    for b in benches:
        md += [f"## {b} - {DESC[b]}", "",
               "| Language | best s | median s | spread | x C (best) | x C (median) | RSS MB |", "|---|---|---|---|---|---|---|"]
        cb = min(times[(b, "c")]); cm = statistics.median(times[(b, "c")])
        for k in KEYS:
            t = times[(b, k)]; bst = min(t); med = statistics.median(t)
            spread = (max(t) - min(t)) / med * 100 if med else 0
            ratios[k].append(bst / cb)
            md.append(f"| {LABEL[k]} | {bst:.3f} | {med:.3f} | {spread:.0f}% | {bst/cb:.2f} | {med/cm:.2f} | {statistics.median(rss[(b,k)])/1024:.1f} |")
        md.append("")
    md += ["## Summary (geometric mean of best-time ratio to C, all benchmarks)", "", "| Language | geomean x C | worst benchmark |", "|---|---|---|"]
    for k in KEYS:
        g = math.exp(sum(math.log(max(r, 1e-9)) for r in ratios[k]) / len(ratios[k]))
        wi = max(range(len(benches)), key=lambda i: ratios[k][i])
        md.append(f"| {LABEL[k]} | {g:.2f} | {benches[wi]} ({ratios[k][wi]:.2f}x) |")
    md += ["", "## Binary size and compile time (mean over benchmarks)", "", "| Language | binary size (KB) | compile time (s) |", "|---|---|---|"]
    for k in KEYS:
        s = statistics.mean(size[(b, k)] for b in benches) / 1024
        c = f"{statistics.mean(ctime[(b, k)] for b in benches):.2f}" if ctime else "n/a (--skip-build)"
        md.append(f"| {LABEL[k]} | {s:.0f} | {c} |")
    md.append("")
    ana = os.path.join(HERE, "ANALYSIS.md")
    if os.path.exists(ana):
        md += [open(ana).read()]
    text = "\n".join(md)
    print(text)
    if not a.no_write:
        open(os.path.join(HERE, "RESULTS.md"), "w").write(text)


main()
