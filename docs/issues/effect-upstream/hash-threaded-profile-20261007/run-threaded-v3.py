#!/usr/bin/env python3
"""Bounded diagnostic capture of original direct-TS Hash cases; no deadline changes."""
import hashlib,json,os,signal,subprocess,sys,time
from pathlib import Path
ROOT=Path(__file__).resolve().parent
SPEC=ROOT/'capture-spec-threaded-v3.json'
def sha(path):
 h=hashlib.sha256()
 with Path(path).open('rb') as f:
  for b in iter(lambda:f.read(1024*1024),b''): h.update(b)
 return h.hexdigest()
def write(path,value):
 temp=path.with_suffix(path.suffix+'.tmp')
 temp.write_text(json.dumps(value,indent=2)+'\n');temp.replace(path)
def terminate_group(child):
 try: os.killpg(child.pid,signal.SIGTERM)
 except ProcessLookupError: pass
 try: child.wait(timeout=10)
 except subprocess.TimeoutExpired:
  try: os.killpg(child.pid,signal.SIGKILL)
  except ProcessLookupError: pass
  child.wait()
 # A terminal leader does not imply its descendants have exited.
 try: os.killpg(child.pid,signal.SIGKILL)
 except ProcessLookupError: pass
s=json.loads(SPEC.read_text())
checks=[(s[k]['path'],s[k]['sha256']) for k in ['binary','runner','sampler']]
checks.append((s['baseline_report']['path'],s['baseline_report']['sha256']))
checks.extend((s['upstream'][k],s['upstream'][k+'_sha256']) for k in ['test','entry','harness'])
def integrity():
 for path,digest in checks:
  if sha(path)!=digest: raise RuntimeError('Input hash mismatch: '+path)
 source=Path(s['upstream']['test']).parents[3]
 if subprocess.check_output(['git','rev-parse','HEAD'],cwd=source,text=True).strip()!=s['upstream']['pin']:
  raise RuntimeError('Upstream pin changed')
 if subprocess.check_output(['git','status','--porcelain','--untracked-files=no'],cwd=source,text=True).strip():
  raise RuntimeError('Tracked upstream source dirty')
integrity()
out=ROOT/('capture-threaded-v3.'+str(time.time_ns()));out.mkdir()
env=os.environ.copy();env.update(s['runtime_config']['environment_from_original_job'])
for key in ['KILN_NATIVE_PROVIDER_MANIFEST','KILN_NATIVE_NUMBER_PROVIDER_MANIFEST','KILN_GC_STRESS','KILN_NATIVE_GC_STRESS']:
 env.pop(key,None)
command=['/usr/bin/python3',s['sampler']['path'],'--focus-exe',s['binary']['path'],'--expected-focus-sha256',s['binary']['sha256'],'--freq','49','--address-limit-bytes','8589934592','--stdout-log',str(out/'raw.log'),'--output',str(out/'profile.json'),'--',s['binary']['path'],'--one-shot','run',s['upstream']['entry'],'--node','--clock','wall']
job={'argv':command,'cwd':str(Path(s['upstream']['harness']).parent),'launcher_sha256':sha(__file__),'spec_sha256':sha(SPEC),'inputs':checks,'outer_timeout_s':2400,'scope':'diagnostic original Hash25; not full acceptance','environment':s['runtime_config']['environment_from_original_job']}
write(out/'job.json',job)
print(out,flush=True)
start=time.monotonic();child=None;code=None;error=None;timeout=False;valid=False
try:
 with (out/'supervisor.log').open('w') as log:
  child=subprocess.Popen(command,cwd=job['cwd'],env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
  write(out/'running.json',{'pid':child.pid,'started_ns':time.time_ns()})
  try: code=child.wait(timeout=2400)
  except subprocess.TimeoutExpired:
   timeout=True;terminate_group(child);code=child.returncode
except BaseException as exc:
 error=repr(exc)
finally:
 if child is not None: terminate_group(child)
 try: integrity();valid=True
 except BaseException as exc: error=(error or '')+'; postflight: '+repr(exc)
 receipt={'terminal':True,'exit_code':code,'elapsed_s':time.monotonic()-start,'timed_out':timeout,'error':error,'input_integrity_valid':valid,'profile_present':(out/'profile.json').exists(),'scope':job['scope'],'output_files':{str(p.relative_to(out)):sha(p) for p in out.iterdir() if p.is_file()}}
 write(out/'status.json',receipt)
 print(json.dumps(receipt,indent=2),flush=True)
sys.exit(0 if code==0 and not timeout and not error and valid else 1)
