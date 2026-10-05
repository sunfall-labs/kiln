import json, os, resource, subprocess, sys, time
label, cwd, output, *argv = sys.argv[1:]
env = dict(os.environ)
env["KILN_EFFECT_DIAGNOSTIC_CASE"] = "Sharding > redelivers a request reset while an asynchronous storage read is pending"
before = resource.getrusage(resource.RUSAGE_CHILDREN)
start = time.monotonic()
with open(output, "wb") as stream:
    result = subprocess.run(argv, cwd=cwd, env=env, stdout=stream, stderr=subprocess.STDOUT)
elapsed = time.monotonic() - start
after = resource.getrusage(resource.RUSAGE_CHILDREN)
receipt = {
    "label": label,
    "argv": argv,
    "cwd": cwd,
    "exit_code": result.returncode,
    "elapsed_seconds": elapsed,
    "child_user_cpu_seconds": after.ru_utime - before.ru_utime,
    "child_system_cpu_seconds": after.ru_stime - before.ru_stime,
    "child_max_rss_kib": after.ru_maxrss,
    "log": output,
}
print(json.dumps(receipt, sort_keys=True))
