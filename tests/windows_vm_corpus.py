#!/usr/bin/env python3
"""Compare PE executables with tests/expected on the local Windows VM."""
import argparse
import base64
from collections import Counter
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import time
import urllib.request
import uuid
import zipfile

ROOT = Path(__file__).resolve().parents[1]
EXPECTED = ROOT / "tests/expected"
# These goldens require POSIX paths, permissions, errors, process commands,
# signals or /proc. Portable goldens still count if the PE backend rejects them.
POSIX = set("""
    env_basic env_process fs_basic fs_buffered fs_handles fs_links io_basic
    hostile_process process_basic process_leaks process_pipes process_timeout
""".split())


def encoded(path):
    return base64.b64encode(path.read_bytes()).decode("ascii")


def pack(out, job, batches, cases):
    batch = f"{job}-{len(batches) + 1}"
    with zipfile.ZipFile(out / (batch + ".zip"), "w", zipfile.ZIP_DEFLATED) as archive:
        for case in cases:
            exe = out / (case["name"] + ".exe")
            archive.write(exe, exe.name)
            exe.unlink()
        archive.writestr("cases.json", json.dumps(cases))
    batches.append((batch, len(cases)))


def stage(out, job, patterns, batch_size, batches):
    cases = []
    built = 0
    rejected = Counter()
    excluded = []
    env = {**os.environ, "YERUX_LOCS": "1"}
    for source in sorted((ROOT / "tests").glob("*.yx")):
        name = source.stem
        golden = EXPECTED / (name + ".out")
        code = golden.with_suffix(".code")
        if not golden.exists() or not code.exists():
            continue
        if patterns and not any(re.search(pattern, name) for pattern in patterns):
            continue
        if name in POSIX:
            excluded.append(name)
            continue
        exe = out / (name + ".exe")
        build = subprocess.run(
            [ROOT / "target/release/yerux", "build", source.relative_to(ROOT),
             "--native", "--target", "windows", "-o", exe],
            env=env, cwd=ROOT, capture_output=True, text=True, timeout=60,
        )
        if build.returncode:
            message = next(iter(build.stderr.splitlines()), f"compiler exited {build.returncode}")
            reason = message.split(" (used by function", 1)[0].split(" is not supported", 1)[0]
            rejected[reason] += 1
            print(f"REJECT {name} {message}", flush=True)
            continue
        args = source.with_suffix(".args")
        stdin = source.with_suffix(".stdin")
        stderr = golden.with_suffix(".stderr")
        cases.append({
            "name": name,
            "arguments": subprocess.list2cmdline(args.read_text().splitlines()) if args.exists() else "",
            "stdin": encoded(stdin) if stdin.exists() else "",
            "stdout": encoded(golden),
            "stderr": encoded(stderr) if stderr.exists() else None,
            "code": int(code.read_text()),
        })
        built += 1
        if len(cases) == batch_size:
            pack(out, job, batches, cases)
            cases.clear()
    if cases:
        pack(out, job, batches, cases)
    print(f"Excluded POSIX goldens ({len(excluded)}): {' '.join(excluded)}", flush=True)
    print(f"Eligible: {built + sum(rejected.values())}, built: {built}, "
          f"rejected: {dict(rejected)}", flush=True)
    return built


def submit(vm, job, script):
    path = vm / "jobs" / (job + ".ps1")
    pending = path.with_suffix(".tmp")
    pending.write_text(script)
    pending.replace(path)
    return vm / "results" / (job + ".txt")


def monitor(vm, command):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(2)
        client.connect(str(vm / "mon.sock"))
        client.recv(4096)
        client.sendall((command + "\n").encode())
        client.recv(4096)


def shutdown(vm, guest, job):
    if guest.poll() is not None:
        print(f"QEMU exited with status {guest.returncode}", flush=True)
        return
    try:
        submit(vm, job + "-shutdown", "Stop-Computer -Force\n")
        guest.wait(timeout=75)
        print("Windows shutdown completed. QEMU exited.", flush=True)
        return
    except (OSError, subprocess.TimeoutExpired) as error:
        print(f"Guest shutdown job failed ({error}). Sending ACPI power button.", flush=True)
    try:
        monitor(vm, "system_powerdown")
        guest.wait(timeout=45)
    except (OSError, subprocess.TimeoutExpired):
        guest.terminate()
        try:
            guest.wait(timeout=15)
        except subprocess.TimeoutExpired:
            guest.kill()
            guest.wait()
    print("QEMU exited after fallback shutdown.", flush=True)


def stop(signum, _frame):
    raise SystemExit(128 + signum)


def run_batch(vm, guest, job, batch, count, deadline):
    script = f"$Archive = 'http://10.0.2.2:8000/files/{job}/{batch}.zip'\n"
    result = submit(vm, batch, script + Path(__file__).with_suffix(".ps1").read_text())
    print(f"Batch {batch}: {count} programs", flush=True)
    stop_at = time.monotonic() + deadline
    while time.monotonic() < stop_at:
        if guest.poll() is not None:
            raise RuntimeError(f"QEMU exited early ({guest.returncode}); see {vm / 'vm.out'}")
        if result.exists():
            output = result.read_text()
            exit_code = re.search(r"^\[exit code: (-?\d+)\]\s*\Z", output, re.MULTILINE)
            if exit_code:
                print(output, flush=True)
                if exit_code[1] != "0":
                    raise RuntimeError(f"Windows VM batch {batch} exited {exit_code[1]}")
                if not re.search(rf"^windows_vm_corpus: complete {count}$", output, re.MULTILINE):
                    raise RuntimeError(f"Windows VM batch {batch} was incomplete")
                return len(re.findall(r"^PASS ", output, re.MULTILINE))
        time.sleep(2)
    raise TimeoutError(f"Windows VM did not finish batch {batch}")


def run(vm, job, batches, deadline):
    if subprocess.run(["pgrep", "-f", r"qemu-system-x86_64.*file=win\.qcow2"],
                      stdout=subprocess.DEVNULL).returncode == 0:
        raise RuntimeError("Windows VM is already running")
    host = None
    guest = None
    try:
        try:
            urllib.request.urlopen("http://127.0.0.1:8000/agent.ps1", timeout=2).close()
        except OSError:
            with (vm / "host.out").open("ab") as log:
                host = subprocess.Popen(["python3", "host.py"], cwd=vm, stdout=log,
                                        stderr=subprocess.STDOUT)
        with (vm / "vm.out").open("ab") as log:
            guest = subprocess.Popen([vm / "run-vm.sh"], cwd=vm, stdout=log, stderr=subprocess.STDOUT)
        return sum(run_batch(vm, guest, job, batch, count, deadline)
                   for batch, count in batches)
    finally:
        try:
            if guest is not None:
                shutdown(vm, guest, job)
        finally:
            if host is not None and host.poll() is None:
                host.terminate()
                host.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("patterns", nargs="*", help="select programs by regex")
    parser.add_argument("--vm", type=Path, default=Path.home() / "vm-windows")
    parser.add_argument("--batch-size", type=int, default=32, help="programs per VM job")
    parser.add_argument("--deadline", type=int, default=720, help="timeout per VM job in seconds")
    args = parser.parse_args()
    if args.batch_size < 1:
        parser.error("--batch-size must be positive")
    vm = args.vm.resolve()
    job = "yerux-corpus-" + uuid.uuid4().hex
    signal.signal(signal.SIGTERM, stop)
    with (vm / "corpus.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        out = vm / "serve" / job
        out.mkdir()
        batches = []
        try:
            count = stage(out, job, args.patterns, args.batch_size, batches)
            if not count:
                return 2
            print(f"Job {job}", flush=True)
            passed = run(vm, job, batches, args.deadline)
            print(f"VM results: {passed}/{count} passed", flush=True)
            return 0 if passed == count else 1
        finally:
            shutil.rmtree(out)
            for name in [batch for batch, _ in batches] + [job + "-shutdown"]:
                for suffix in (".ps1", ".ps1.taken", ".tmp"):
                    (vm / "jobs" / (name + suffix)).unlink(missing_ok=True)


if __name__ == "__main__":
    raise SystemExit(main())
