import hashlib, json, os, subprocess, sys, time
from pathlib import Path
root=Path('~/Work/kiln/.scratch/effect-sandbox-integration')
out=root/'.scratch/hash-interpreter-profile-d4046'
binary=root/'.scratch/hash-vm-classification-replay/toolchain/kiln-d4046b9e5d4bceb6903aca62e154b5782a7c7831a9bacb90b664e0495c04d655'
expected='d4046b9e5d4bceb6903aca62e154b5782a7c7831a9bacb90b664e0495c04d655'
actual=hashlib.file_digest(binary.open('rb'),'sha256').hexdigest()
if actual!=expected: raise SystemExit('binary digest mismatch')
workspace=root/'.scratch/frozen-clock-native/workspace'
cli=[str(binary),'--one-shot','run',str(workspace/'files/Hash/entry.ts'),'--node','--clock','wall']
command=['~/.cargo/bin/samply','record','--save-only','--unstable-presymbolicate','--duration','90','--output',str(out/'profile.json.gz'),'--','/usr/bin/python3',str(root/'scripts/effect-upstream.py'),'--limited-child','4096',*cli]
env=dict(os.environ,TZ='UTC',KILN_JOBS='1',RAYON_NUM_THREADS='1')
for key in ('KILN_GC_STRESS','KILN_NATIVE_GC_STRESS'): env.pop(key,None)
started=time.monotonic(); records=[]
with (out/'profiled-run.log').open('wb') as raw,(out/'markers.jsonl').open('w') as markers:
    child=subprocess.Popen(command,cwd=workspace,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
    for line in child.stdout:
        raw.write(line); raw.flush()
        sys.stdout.buffer.write(line); sys.stdout.buffer.flush()
        if line.startswith(b'KILN_EFFECT '):
            record=json.loads(line[len(b'KILN_EFFECT '):]); records.append(record)
            markers.write(json.dumps({'elapsed_s':time.monotonic()-started,'monotonic_ns':time.monotonic_ns(),'record':record})+'\n'); markers.flush()
    code=child.wait()
receipt={'binary_sha256':actual,'interpreter_exit_code':code,'profile_tool':'samply record --save-only --unstable-presymbolicate --duration 90','wall_s':time.monotonic()-started,'command':command,'cwd':str(workspace),'summary':[r for r in records if r.get('kind')=='summary'],'record_count':len(records),'original_case_deadlines':'unchanged; sampling is diagnostic only','source_scope':'frozen d4046b9e CLI, source ecfc065b; not final combined CLI','unsampled_acceptance_report':'../hash-vm-classification-replay/report.json'}
(out/'profiled-run-status.json').write_text(json.dumps(receipt,indent=2)+'\n')
raise SystemExit(code)
