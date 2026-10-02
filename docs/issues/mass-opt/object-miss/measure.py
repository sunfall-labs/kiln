import collections, hashlib, json, os, pathlib, subprocess, sys, time
base=pathlib.Path(__file__).resolve().parent
binary=pathlib.Path(sys.argv[1]).resolve(); label=sys.argv[2]
run=base/label; run.mkdir()
project=run/'project'; project.mkdir()
(project/'package.json').write_text('{"type":"module"}\n')
(project/'main.js').write_text('import {answer} from "./dep.js"; console.log(answer);\n')
(project/'dep.js').write_text('export const answer = 42;\n')
wrapper=base/'cc'
wrapper.write_text('''#!/usr/bin/env bash
compile=0
source_name=unknown
for arg in "$@"; do
  [[ $arg == -c ]] && compile=1
  [[ $arg == *.c ]] && source_name=${arg##*/}
done
if (( compile )); then printf '%s\\n' "$source_name" >> "$KILN_OBJECT_COUNT_LOG"; fi
exec ~/.cache/kiln/clang-18/usr/bin/clang-18 "$@"
''')
wrapper.chmod(0o755)
env=dict(os.environ, CC=str(wrapper), KILN_JOBS='1', RAYON_NUM_THREADS='1',
 KILN_SHARED_CACHE=str(run/'cache'), KILN_OBJECT_COUNT_LOG=str(run/'compiles.log'),
 LD_LIBRARY_PATH='~/.cache/kiln/clang-18/usr/lib/x86_64-linux-gnu:~/.cache/kiln/clang-18/usr/lib/llvm-18/lib')
cpu_before=os.times()
started=time.monotonic()
processes=[subprocess.Popen([str(binary),'--one-shot','run','--native','main.js'],cwd=project,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE) for _ in range(4)]
results=[]
for index,p in enumerate(processes):
 out,err=p.communicate()
 (run/f'{index}.stdout').write_bytes(out); (run/f'{index}.stderr').write_bytes(err)
 results.append({'status':p.returncode,'stdout':out.decode()})
elapsed=time.monotonic()-started
cpu_after=os.times()
counts=collections.Counter((run/'compiles.log').read_text().splitlines())
report={'label':label,'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'wall_seconds':elapsed,'child_cpu_seconds':(cpu_after.children_user+cpu_after.children_system)-(cpu_before.children_user+cpu_before.children_system),'compile_counts':dict(counts),'total_compiles':sum(counts.values()),'results':results,'workers_per_process':1,'processes':4,'cache':'fresh, shared only among these four invocations'}
(run/'result.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report))
assert all(r=={'status':0,'stdout':'42\n'} for r in results),results
