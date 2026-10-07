import hashlib, json, os, subprocess, sys, time
from pathlib import Path
root=Path('~/Work/kiln/.scratch/effect-sandbox-integration')
out=Path('~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/date-native-milestone/.scratch/hash-current-vm-profile')
binary=Path('~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/date-native-milestone/.scratch/final-native-milestone/corrected-gate/kiln-cdae5bcf3bd90f73412e0e0b3d35d0cefc8204fb388502476673d0e700ae6df2')
expected='cdae5bcf3bd90f73412e0e0b3d35d0cefc8204fb388502476673d0e700ae6df2'
actual=hashlib.file_digest(binary.open('rb'),'sha256').hexdigest()
if actual!=expected: raise SystemExit('binary digest mismatch')
workspace=root/'.scratch/frozen-clock-native/workspace'
cmd=['/usr/bin/python3',str(root/'scripts/effect-upstream.py'),'--limited-child','4096',str(binary),'--one-shot','run',str(workspace/'files/Hash/entry.ts'),'--node','--clock','wall']
env=dict(os.environ,TZ='UTC',KILN_JOBS='1',RAYON_NUM_THREADS='1')
for key in ('KILN_GC_STRESS','KILN_NATIVE_GC_STRESS'): env.pop(key,None)
started=time.monotonic(); records=[]
with (out/'target-run.log').open('wb') as raw,(out/'target-markers.jsonl').open('w') as markers:
    child=subprocess.Popen(cmd,cwd=workspace,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
    (out/'target-running.json').write_text(json.dumps({'pid':child.pid,'started_unix_s':time.time(),'binary_sha256':actual,'command':cmd,'cwd':str(workspace)},indent=2)+'\n')
    for line in child.stdout:
        raw.write(line); raw.flush()
        sys.stdout.buffer.write(line); sys.stdout.buffer.flush()
        if line.startswith(b'KILN_EFFECT '):
            record=json.loads(line[len(b'KILN_EFFECT '):]); records.append(record)
            markers.write(json.dumps({'elapsed_s':time.monotonic()-started,'monotonic_ns':time.monotonic_ns(),'record':record})+'\n'); markers.flush()
    code=child.wait()
receipt={'binary_sha256':actual,'interpreter_exit_code':code,'wall_s':time.monotonic()-started,'command':cmd,'cwd':str(workspace),'summary':[r for r in records if r.get('kind')=='summary'],'record_count':len(records),'original_case_deadlines':'unchanged; sampling is diagnostic only','source_scope':'pushed production CLI cdae5bcf, source f53e22c6; diagnostic under compiler contention; no acceptance claim','unsampled_acceptance_report':None}
(out/'target-run-status.json').write_text(json.dumps(receipt,indent=2)+'\n')
raise SystemExit(code)
