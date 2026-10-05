import hashlib, json, os, pathlib, resource, subprocess, time
root = pathlib.Path(__file__).resolve().parent
case = "Sharding > redelivers a request reset while an asynchronous storage read is pending"
bins = [
    ("baseline", root / "sharding-baseline.bin"),
    ("candidate", root / "sharding-candidate.bin"),
]
sequence = [("baseline", 1), ("candidate", 1), ("baseline", 2), ("candidate", 2), ("baseline", 3), ("candidate", 3)]
env = os.environ.copy()
env["KILN_EFFECT_DIAGNOSTIC_CASE"] = case
env.pop("KILN_NATIVE_GC_STRESS", None)
records=[]
for name, repetition in sequence:
    binary = dict(bins)[name]
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.perf_counter()
    try:
        result = subprocess.run([str(binary)], cwd="~/Work/kiln-effect-allocator-full-stress/.scratch/sharding-counterprobe-overlay", env=env, capture_output=True, text=True, timeout=15)
        rc, out, err, timed_out = result.returncode, result.stdout, result.stderr, False
    except subprocess.TimeoutExpired as exc:
        rc, timed_out = None, True
        out = (exc.stdout or b"").decode(errors="replace") if isinstance(exc.stdout, bytes) else (exc.stdout or "")
        err = (exc.stderr or b"").decode(errors="replace") if isinstance(exc.stderr, bytes) else (exc.stderr or "")
    elapsed = time.perf_counter() - start
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    record = {
        "engine": name,
        "repetition": repetition,
        "case": case,
        "binary": str(binary),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "external_wall_seconds": elapsed,
        "child_user_cpu_seconds": after.ru_utime - before.ru_utime,
        "child_system_cpu_seconds": after.ru_stime - before.ru_stime,
        "returncode": rc,
        "outer_timeout": timed_out,
        "stdout": out,
        "stderr": err,
    }
    records.append(record)
    (root / f"{name}-{repetition}.json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps({k: record[k] for k in ("engine", "repetition", "external_wall_seconds", "child_user_cpu_seconds", "child_system_cpu_seconds", "returncode", "outer_timeout")}), flush=True)
summary={"selected_case":case,"deadline":"original unchanged 5000 ms in harness","outer_safety_timeout_seconds":15,"gc_stress":"unset","sequence":[f"{n}-{i}" for n,i in sequence],"records":records}
(root / "timing-receipt.json").write_text(json.dumps(summary, indent=2)+"\n")
