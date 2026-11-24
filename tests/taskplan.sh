#!/usr/bin/env bash
# examples/real/taskplan: unit tests on both backends; main built on both backends; exact output of check, plan (with
# and without --workers), dot and json on a fixed example against goldens from an independent Python 3 model; C and
# native output identical for every command; error messages and exit statuses; a seeded corpus of 80 random graphs
# (some with an injected cycle) whose check and plan output must match the model on both backends. Exit 0 when
# everything passes. Nothing is written inside the repository.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/taskplan
fail=0
for flag in "" --native; do
  "$Y" test $D/tests.yx $flag >"$T/t.out" 2>&1 || { echo "FAIL taskplan unit tests ($flag)"; tail -5 "$T/t.out"; fail=1; }
done
"$Y" build $D/main.yx -o "$T/tp-c" >"$T/b1.out" 2>&1 || { echo "FAIL taskplan build (c)"; cat "$T/b1.out"; fail=1; }
"$Y" build $D/main.yx --native -o "$T/tp-n" >"$T/b2.out" 2>&1 || { echo "FAIL taskplan build (native)"; cat "$T/b2.out"; fail=1; }
[ $fail = 0 ] || exit 1

# The independent model: writes the fixed example, its goldens and the random corpus with expected outputs.
cat >"$T/model.py" <<'PY'
import heapq, json, random, sys

T = sys.argv[1]

class Bad(Exception):
    def __init__(self, msg):
        self.msg = msg

def analyse(tasks):
    """Validate and schedule. Raises Bad with the message of the first error."""
    ids = sorted(t["id"] for t in tasks)
    for a, b in zip(ids, ids[1:]):
        if a == b:
            raise Bad(f'duplicate id "{a}"')
    by = {t["id"]: t for t in tasks}
    deps = {}
    for i in ids:
        deps[i] = []
        for d in by[i].get("deps", []):
            if d == i:
                raise Bad(f'task "{i}": depends on itself')
            if d not in by:
                raise Bad(f'task "{i}": unknown dependency "{d}"')
            deps[i].append(d)
    succ = {i: [] for i in ids}
    for i in ids:
        for d in deps[i]:
            succ[d].append(i)
    indeg = {i: len(deps[i]) for i in ids}
    heap = [i for i in ids if indeg[i] == 0]
    heapq.heapify(heap)
    order = []
    while heap:
        i = heapq.heappop(heap)
        order.append(i)
        for s in succ[i]:
            indeg[s] -= 1
            if indeg[s] == 0:
                heapq.heappush(heap, s)
    if len(order) < len(ids):
        remaining = set(ids) - set(order)
        path = [min(remaining)]
        while True:
            nxt = min(d for d in deps[path[-1]] if d in remaining)
            if nxt in path:
                cyc = path[path.index(nxt):] + [nxt]
                break
            path.append(nxt)
        raise Bad("cycle: " + " -> ".join(cyc))
    dur = {t["id"]: t["duration"] for t in tasks}
    es, ef = {}, {}
    for i in order:
        es[i] = max((ef[d] for d in deps[i]), default=0)
        ef[i] = es[i] + dur[i]
    makespan = max(ef.values(), default=0)
    lf = {i: makespan for i in ids}
    for i in reversed(order):
        for s in succ[i]:
            lf[i] = min(lf[i], lf[s] - dur[s])
    slack = {i: lf[i] - ef[i] for i in ids}
    path = []
    if ids:
        cur = next(i for i in ids if ef[i] == makespan)
        path = [cur]
        while es[cur] > 0:
            cur = min(d for d in deps[cur] if ef[d] == es[cur])
            path.append(cur)
        path.reverse()
    return dict(ids=ids, order=order, deps=deps, succ=succ, dur=dur, es=es, ef=ef,
                slack=slack, makespan=makespan, path=path)

def plan_text(m):
    out = ""
    for i in m["order"]:
        mark = "yes" if m["slack"][i] == 0 else "no"
        out += f'{i} {m["es"][i]} {m["ef"][i]} {m["slack"][i]} {mark}\n'
    out += f'makespan {m["makespan"]}\n'
    out += ("path " + " -> ".join(m["path"]) + "\n") if m["path"] else "path (none)\n"
    return out

def simulate(m, workers):
    """list schedule on identical workers: at each event time the ready tasks, in id order, take the lowest free
    workers; a worker is released when its task finishes."""
    dur, deps, ids = m["dur"], m["deps"], m["ids"]
    start, finish = {}, {}
    lanes = [[] for _ in range(workers)]
    free = set(range(workers))
    running = []
    t = 0
    while len(start) < len(ids):
        for (f, w, i) in list(running):
            if f == t:
                free.add(w)
                running.remove((f, w, i))
        ready = sorted(i for i in ids if i not in start
                       and all(d in finish and finish[d] <= t for d in deps[i]))
        for i in ready:
            if not free:
                break
            w = min(free)
            free.discard(w)
            start[i] = t
            finish[i] = t + dur[i]
            lanes[w].append(i)
            running.append((finish[i], w, i))
        if len(start) == len(ids):
            break
        t = min(f for (f, _, _) in running)
    return dict(start=start, finish=finish, lanes=lanes,
                makespan=max(finish.values(), default=0))

def list_text(sim, workers):
    out = f"workers {workers} makespan {sim['makespan']}\n"
    for k in range(workers):
        out += f"worker {k}:" + "".join(
            f" {i}@{sim['start'][i]}-{sim['finish'][i]}" for i in sim["lanes"][k]) + "\n"
    return out

def plan_json(m, workers=0, sim=None):
    tasks = [dict(id=i, start=m["es"][i], finish=m["ef"][i], slack=m["slack"][i],
                  critical=m["slack"][i] == 0) for i in m["order"]]
    doc = dict(tasks=tasks, makespan=m["makespan"], critical_path=m["path"])
    if workers:
        doc["workers"] = dict(count=workers, makespan=sim["makespan"], timeline=[
            dict(worker=k, tasks=[dict(id=i, start=sim["start"][i], finish=sim["finish"][i]) for i in lane])
            for k, lane in enumerate(sim["lanes"])])
    return json.dumps(doc, indent=2, ensure_ascii=False) + "\n"

def write_case(stem, tasks, workers):
    """expected outputs of check and plan for one document, as the program prints them."""
    path = f"{T}/{stem}.json"
    open(f"{T}/{stem}.w", "w").write(f"{workers}\n")
    with open(path, "w") as f:
        json.dump({"tasks": tasks}, f, indent=1)
    try:
        m = analyse(tasks)
    except Bad as e:
        for tag in ("check", "plan", "planw"):
            open(f"{T}/{stem}.{tag}", "w").write("")
            open(f"{T}/{stem}.{tag}.err", "w").write(f"taskplan: {path}: {e.msg}\n")
            open(f"{T}/{stem}.{tag}.st", "w").write("1\n")
        return
    ok = f"ok {len(tasks)} tasks\n"
    text = plan_text(m)
    planw = text + list_text(simulate(m, workers), workers)
    for tag, out in (("check", ok), ("plan", text), ("planw", planw)):
        open(f"{T}/{stem}.{tag}", "w").write(out)
        open(f"{T}/{stem}.{tag}.err", "w").write("")
        open(f"{T}/{stem}.{tag}.st", "w").write("0\n")

# ---- the fixed example: every command has a golden ----
fixed = [
    {"id": "release", "duration": 1, "deps": ["test", "package", "docs"]},
    {"id": "fetch", "duration": 2},
    {"id": "lint", "duration": 1, "deps": ["fetch"]},
    {"id": "compile", "duration": 4, "deps": ["fetch"]},
    {"id": "test", "duration": 3, "deps": ["compile"]},
    {"id": "docs", "duration": 2, "deps": ["fetch"]},
    {"id": "package", "duration": 2, "deps": ["compile", "lint"]},
]
with open(f"{T}/fixed.json", "w") as f:
    json.dump({"tasks": fixed}, f, indent=1)
m = analyse(fixed)
sim2 = simulate(m, 2)
open(f"{T}/fixed.check", "w").write(f"ok {len(fixed)} tasks\n")
open(f"{T}/fixed.plan", "w").write(plan_text(m))
open(f"{T}/fixed.planw", "w").write(plan_text(m) + list_text(sim2, 2))
open(f"{T}/fixed.json.want", "w").write(plan_json(m, 2, sim2))
dot = ['digraph taskplan {']
for i in m["ids"]:
    dot.append(f'  "{i}" [label="{i} ({m["dur"][i]})"];')
for i in m["ids"]:
    for d in sorted(m["deps"][i]):
        dot.append(f'  "{d}" -> "{i}";')
dot.append('}')
open(f"{T}/fixed.dot", "w").write("\n".join(dot) + "\n")

# ---- the corpus: random DAGs, every 4th with an injected cycle (a 1-task one becomes a self dependency) ----
rng = random.Random(7)
pool = ["a", "b", "t", "x-", "_q", "job", "k9", "a1", "a-1", "a_1", "build", "build_2", "0", "z"]
for c in range(80):
    n = rng.randint(1, 12)
    ids = set()
    while len(ids) < n:
        ids.add(rng.choice(pool) + str(rng.randint(0, 30)))
    order = list(ids)
    rng.shuffle(order)
    deps = {order[k]: [order[j] for j in range(k) if rng.random() < 0.3] for k in range(n)}
    if c % 4 == 3:
        if n == 1:
            deps[order[0]] = [order[0]]
        else:
            u, v = rng.sample(order, 2)
            deps[u] = list(dict.fromkeys(deps[u] + [v]))
            deps[v] = list(dict.fromkeys(deps[v] + [u]))
    tasks = []
    for i in order:
        t = {"id": i, "duration": rng.randint(1, 9)}
        if deps[i]:
            t["deps"] = list(deps[i])
        tasks.append(t)
    rng.shuffle(tasks)
    write_case(f"c{c}", tasks, rng.randint(1, 4))
PY
python3 "$T/model.py" "$T" || { echo "FAIL taskplan model"; exit 1; }

for b in c n; do
  "$T/tp-$b" check "$T/fixed.json" >"$T/fx.check.$b" 2>&1 ; st=$?
  [ "$st" = 0 ] && cmp -s "$T/fixed.check" "$T/fx.check.$b" || { echo "FAIL taskplan check fixed ($b)"; fail=1; }
  "$T/tp-$b" plan "$T/fixed.json" >"$T/fx.plan.$b" 2>&1
  cmp -s "$T/fixed.plan" "$T/fx.plan.$b" || { echo "FAIL taskplan plan fixed ($b)"; diff "$T/fixed.plan" "$T/fx.plan.$b"; fail=1; }
  "$T/tp-$b" plan "$T/fixed.json" --workers 2 >"$T/fx.planw.$b" 2>&1
  cmp -s "$T/fixed.planw" "$T/fx.planw.$b" || { echo "FAIL taskplan plan --workers 2 fixed ($b)"; diff "$T/fixed.planw" "$T/fx.planw.$b"; fail=1; }
  "$T/tp-$b" dot "$T/fixed.json" >"$T/fx.dot.$b" 2>&1
  cmp -s "$T/fixed.dot" "$T/fx.dot.$b" || { echo "FAIL taskplan dot fixed ($b)"; diff "$T/fixed.dot" "$T/fx.dot.$b"; fail=1; }
  "$T/tp-$b" json "$T/fixed.json" --workers 2 >"$T/fx.json.$b" 2>&1
  cmp -s "$T/fixed.json.want" "$T/fx.json.$b" || { echo "FAIL taskplan json fixed ($b)"; diff "$T/fixed.json.want" "$T/fx.json.$b" | head -20; fail=1; }
done
"$T/tp-c" json "$T/fixed.json" --workers 2 >"$T/jc" && "$T/tp-n" json "$T/fixed.json" --workers 2 >"$T/jn"
cmp -s "$T/jc" "$T/jn" || { echo "FAIL taskplan json C/native differ"; fail=1; }
[ $fail = 0 ] && echo "taskplan: fixed example matches the model (check, plan, plan --workers, dot, json)"

# Errors: stdout empty, stderr and status exact, on both backends.
printf '{"tasks": [\n  {"id": "a" "duration": 1}\n]}\n' >"$T/badjson.json"
printf '{"tasks": [\n  {"id": "a", "duration": 1},\n  ]}\n' >"$T/trailing.json"
printf '{"tasks": [{"id": "a", "duration": 1, "deps": ["zz"]}]}' >"$T/unk.json"
printf '{"tasks": [{"id": "a", "duration": 1}, {"id": "a", "duration": 2}]}' >"$T/dup.json"
printf '{"tasks": [{"id": "a", "duration": 1, "deps": ["a"]}]}' >"$T/self.json"
printf '{"tasks": [{"id": "Bad", "duration": 1}]}' >"$T/badid.json"
printf '{"tasks": [{"id": "a", "duration": 1.5}]}' >"$T/baddur.json"
printf '{"tasks": [{"id": "a", "duration": 1, "deps": ["b"]}, {"id": "b", "duration": 1, "deps": ["c"]}, {"id": "c", "duration": 1, "deps": ["a"]}]}' >"$T/cyc.json"
printf '{"items": []}' >"$T/nolist.json"
err() { # name, want status, want stderr, args...
  local name=$1 want=$2 wanterr=$3; shift 3
  for b in c n; do
    "$T/tp-$b" "$@" >"$T/e.out.$b" 2>"$T/e.err.$b"; local st=$?
    [ "$st" = "$want" ] || { echo "FAIL taskplan $name status ($b): $st, want $want"; cat "$T/e.err.$b"; fail=1; }
    [ -s "$T/e.out.$b" ] && { echo "FAIL taskplan $name: stdout not empty ($b)"; fail=1; }
    [ "$(cat "$T/e.err.$b")" = "$wanterr" ] || { echo "FAIL taskplan $name message ($b)"; cat "$T/e.err.$b"; fail=1; }
  done
}
err badjson 1 "taskplan: $T/badjson.json:2:14: expected ',' or '}'" check "$T/badjson.json"
err trailing 1 "taskplan: $T/trailing.json:3:3: unexpected character" plan "$T/trailing.json"
err unknown-dep 1 "taskplan: $T/unk.json: task \"a\": unknown dependency \"zz\"" check "$T/unk.json"
err duplicate 1 "taskplan: $T/dup.json: duplicate id \"a\"" plan "$T/dup.json" --workers 2
err self 1 "taskplan: $T/self.json: task \"a\": depends on itself" json "$T/self.json"
err badid 1 "taskplan: $T/badid.json: bad id \"Bad\": use only a-z, 0-9, _ and -" check "$T/badid.json"
err baddur 1 "taskplan: $T/baddur.json: task \"a\": duration must be a positive integer" check "$T/baddur.json"
err cycle 1 "taskplan: $T/cyc.json: cycle: a -> b -> c -> a" plan "$T/cyc.json"
err cycle-dot 1 "taskplan: $T/cyc.json: cycle: a -> b -> c -> a" dot "$T/cyc.json"
err nolist 1 "taskplan: $T/nolist.json: top level must be an object with a \"tasks\" array" check "$T/nolist.json"
err nofile 1 "taskplan: $T/none.json: cannot read: open '$T/none.json': No such file or directory (os error 2)" check "$T/none.json"
err bogus 2 "taskplan: unknown command 'bogus' (use check, plan, dot or json)" bogus "$T/fixed.json"
err workers0 2 "taskplan: --workers needs a positive integer, got '0'" plan "$T/fixed.json" --workers 0
err workersx 2 "taskplan: --workers needs a positive integer, got 'x'" json "$T/fixed.json" --workers x
err workers-check 2 "taskplan: --workers is only for plan and json" check "$T/fixed.json" --workers 2
err option 2 "taskplan: unknown option '--w' (the option is --workers W)" plan "$T/fixed.json" --w 2
err usage 2 "taskplan: usage: taskplan check|plan|dot|json FILE [--workers W]"
err usage-extra 2 "taskplan: usage: taskplan check|plan|dot|json FILE [--workers W]" check "$T/fixed.json" extra
[ $fail = 0 ] && echo "taskplan: error messages and statuses match"

# Corpus: check and plan (and plan --workers W) on both backends against the model.
mism=0; errs=0
for c in $(seq 0 79); do
  w=$(cat "$T/c$c.w" 2>/dev/null || echo 1)
  for tag in check plan planw; do
    extra=()
    [ $tag = planw ] && extra=(--workers "$w")
    cmd=plan
    [ $tag = check ] && cmd=check
    for b in c n; do
      "$T/tp-$b" "$cmd" "$T/c$c.json" "${extra[@]}" >"$T/rc.out" 2>"$T/rc.err"; st=$?
      if [ "$st" != "$(cat "$T/c$c.$tag.st")" ] || ! cmp -s "$T/c$c.$tag" "$T/rc.out" || ! cmp -s "$T/c$c.$tag.err" "$T/rc.err"; then
        mism=$((mism + 1)); [ $mism -le 3 ] && { echo "taskplan corpus differs: c$c $tag ($b), status $st"; cat "$T/rc.err"; }
      fi
    done
    [ "$(cat "$T/c$c.$tag.st")" = 1 ] && errs=$((errs + 1))
  done
done
[ $mism = 0 ] || { echo "FAIL taskplan corpus: $mism mismatches"; fail=1; }
[ $fail = 0 ] && echo "taskplan: ok (unit tests on both backends, fixed example, error cases, corpus of 80 graphs: $((errs / 3)) rejected, both backends match the model)"
exit $fail
