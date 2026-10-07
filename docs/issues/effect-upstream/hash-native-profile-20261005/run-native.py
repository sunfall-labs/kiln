import hashlib, json, os, subprocess, sys, time
from pathlib import Path
root=Path('~/Work/kiln/.scratch/effect-sandbox-integration')
out=root/'.scratch/hash-native-profile'
binary=out/'native.bin'
expected='2192603b13d2ba0687d80de5d5955ba5518d4b2b6108d93b291934cf2ed45297'
actual=hashlib.file_digest(binary.open('rb'),'sha256').hexdigest()
if actual!=expected: raise SystemExit('binary digest mismatch')
workspace=root/'.scratch/suppressed-newtarget/.scratch/full-native-workspace'
cmd=[sys.executable,str(root/'scripts/effect-upstream.py'),'--limited-child','8192',str(binary)]
env=dict(os.environ,TZ='UTC',KILN_JOBS='1',RAYON_NUM_THREADS='1')
for key in ('KILN_GC_STRESS','KILN_NATIVE_GC_STRESS'): env.pop(key,None)
started=time.monotonic()
child=subprocess.Popen(cmd,cwd=workspace,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
records=[]
with (out/'native.log').open('wb') as raw,(out/'markers.jsonl').open('w') as markers:
    for line in child.stdout:
        raw.write(line); raw.flush()
        sys.stdout.buffer.write(line); sys.stdout.buffer.flush()
        if line.startswith(b'KILN_EFFECT '):
            record=json.loads(line[len(b'KILN_EFFECT '):])
            records.append(record)
            markers.write(json.dumps({'elapsed_s':time.monotonic()-started,'monotonic_ns':time.monotonic_ns(),'record':record})+'\n'); markers.flush()
code=child.wait()
receipt={'binary_sha256':actual,'native_exit_code':code,'wall_s':time.monotonic()-started,'command':cmd,'cwd':str(workspace),'summary':[r for r in records if r.get('kind')=='summary'],'record_count':len(records),'original_case_deadlines':'unchanged; diagnostics under sampling and concurrent native survey','source_scope':'old frozen survey CLI5458, not final integration'}
(out/'native-status.json').write_text(json.dumps(receipt,indent=2)+'\n')
raise SystemExit(code)
