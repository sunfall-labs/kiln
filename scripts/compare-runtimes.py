#!/usr/bin/env python3
"""Matched native-runtime probes and startup/steady-state benchmarks.

Toolchains are supplied explicitly; no installation, downloads or source changes occur here.
All compile and execution outcomes remain in the report. Timings require Node-verified answers.
"""
import argparse
import hashlib
import json
import os
import platform
import random
import shutil
import signal
import statistics
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--out',required=True)
parser.add_argument('--work',required=True,help='private artifact and raw-log directory')
parser.add_argument('--passes',type=int,default=7)
parser.add_argument('--cpu',type=int,default=None,help='pin timed executions to one allowed logical CPU')
parser.add_argument('--runtimes',default='node,bun,kiln,perry,porffor,static-hermes')
parser.add_argument('--only',help='one workload or compatibility probe for diagnosis')
args=parser.parse_args()
if args.passes<3:parser.error('at least three passes are required')
work=Path(args.work).resolve();work.mkdir(parents=True,exist_ok=True)
env=dict(os.environ,TZ='UTC',NO_COLOR='1',KILN_JOBS='2',RAYON_NUM_THREADS='2',PERRY_SKIP_CODEGEN='1')
tools={name:os.environ.get(var,shutil.which(default) or '') for name,var,default in [('node','NODE_BIN','node'),('bun','BUN_BIN','bun'),('kiln','KILN_BIN','kiln'),('perry','PERRY_BIN','perry'),('porffor','PORF_BIN','porf'),('static-hermes','SHERMES_BIN','shermes'),('esbuild','ESBUILD_BIN','esbuild')]}
selected=args.runtimes.split(',')
if len(selected)!=len(set(selected)) or any(x not in tools or x=='esbuild' for x in selected):parser.error('unknown or duplicate runtime')
if 'node' not in selected:parser.error('Node is required as the oracle')
selected=['node',*[name for name in selected if name!='node']]
if args.cpu is not None and args.cpu not in os.sched_getaffinity(0):parser.error('requested CPU is outside process affinity')

def sha(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()
def invoke(command,timeout=60,pin=False):
    if pin and args.cpu is not None:command=['taskset','-c',str(args.cpu),*command]
    start=time.perf_counter()
    p=subprocess.Popen(command,cwd=ROOT,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True)
    timed_out=False
    try:
        stdout,stderr=p.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out=True
        os.killpg(p.pid,signal.SIGKILL)
        stdout,stderr=p.communicate()
    result={'exit_code':None if timed_out else p.returncode,'wall_ms':None if timed_out else (time.perf_counter()-start)*1000,'stdout':stdout.decode('utf-8',errors='replace'),'stdout_sha256':hashlib.sha256(stdout).hexdigest(),'stderr':stderr.decode('utf-8',errors='replace'),'command':command}
    if timed_out:result['timeout']=True
    return result

def build(runtime,path,case_id,bundle=False):
    tool=tools[runtime]
    if not tool or not Path(tool).is_file():return {'status':'toolchain-missing'},None
    src=ROOT/path
    if bundle or runtime=='static-hermes':
        if not tools['esbuild']:return {'status':'preparation-failed','reason':'esbuild missing'},None
        transformed=work/f'{case_id}-{runtime}.js'
        prep=[tools['esbuild'],str(src),'--target=esnext','--format=esm','--outfile='+str(transformed)]
        if bundle:prep+=['--bundle','--platform=neutral']
        result=invoke(prep)
        if result['exit_code']!=0:return {'status':'preparation-failed','preparation':result},None
        src=transformed
    if runtime in ('node','bun'):return {'status':'ready'},[tool,str(src)]
    output=work/f'{runtime}-{case_id}'
    if runtime=='kiln':command=[tool,'--one-shot','build',str(src),'--profile','native-release','--output',str(output)]
    elif runtime=='perry':command=[tool,'compile',str(src),'-o',str(output)]
    elif runtime=='porffor':command=[tool,'native','-O3',str(src),'-o',str(output)]
    else:command=[tool,'-O',str(src),'-o',str(output)]
    result=invoke(command,timeout=600)
    (work/f'{runtime}-{case_id}.build.json').write_text(json.dumps(result,indent=2)+'\n')
    if result['exit_code']!=0 or not output.is_file():return {'status':'compile-timeout' if result.get('timeout') else 'compile-error','build':result},None
    return {'status':'ready','build':result,'artifact_bytes':output.stat().st_size,'artifact_sha256':sha(output)},[str(output)]

metadata={}
for name in selected:
    binary=tools[name]
    metadata[name]={'binary_sha256':sha(binary) if binary and Path(binary).is_file() else None,'version':invoke([binary,'--version'])['stdout'].strip() if binary and Path(binary).is_file() else None}
metadata['kiln']['release']='v0.1.0'
if 'static-hermes' in metadata:metadata['static-hermes']['source_commit']=os.environ.get('SHERMES_SOURCE_REV')
for runtime in ['perry','porffor']:
    release=ROOT/'.scratch/comparison-toolchains'/f'{runtime}-release.json'
    if runtime in metadata and release.is_file():metadata[runtime]['release_record']=json.loads(release.read_text())
manifest=json.loads((ROOT/'benchmarks/compat/manifest.json').read_text())
if len({c['id'] for c in manifest})!=len(manifest):raise SystemExit('duplicate compatibility case')
programs=[('startup','cli'),('json-transform','cli'),('template-render','cli'),('warm-numeric','steady'),('warm-json','steady')]
if args.only and args.only not in {c['id'] for c in manifest}|{p[0] for p in programs}:parser.error('unknown probe')
report={'schema':1,'date':datetime.now(timezone.utc).isoformat(),'machine':{'os':platform.platform(),'cpu':next((line.split(':',1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')),platform.processor()),'logical_cpus':os.cpu_count(),'timing_cpu':args.cpu,'load_before':list(os.getloadavg())},'method':{'passes':args.passes,'interleaving_seed':42,'compile_timeout_s':600,'execution_timeout_s':60,'warmup_calls':200,'calibration_floor_ms':100,'cli':'external wall time includes spawn, startup and work; excludes compilation','steady':'Date.now inside native/process program; 200 warmup calls, calibrated batches; exact checksum of every timed result checked against Node','npm':'same esbuild neutral bundle for every runtime; package APIs tested, not runtime package resolution','profiles':{'kiln':'native-release','perry':'default native, prebuilt full stdlib; no V8 fallback flag','porffor':'native -O3','static-hermes':'shermes -O; native executable dynamically links Hermes runtime on Linux; esbuild strips TS annotations'},'environment':{'TZ':'UTC','NO_COLOR':'1','KILN_JOBS':'2','PERRY_SKIP_CODEGEN':'1'}},'harness_sha256':sha(__file__),'manifest_sha256':sha(ROOT/'benchmarks/compat/manifest.json'),'npm_lock_sha256':sha(ROOT/'benchmarks/package-lock.json'),'toolchains':metadata,'npm_versions':json.loads((ROOT/'benchmarks/package.json').read_text())['dependencies'],'compatibility':[],'benchmarks':[]}

for case in manifest:
    if args.only and case['id']!=args.only:continue
    row={**case,'source_sha256':sha(ROOT/case['path']),'runtimes':{}}
    expected=None
    for runtime in selected:
        outcome,command=build(runtime,case['path'],case['id'],case['group']=='npm')
        if command:
            result=invoke(command)
            outcome['execution']=result
            if runtime=='node':
                if result['exit_code']!=0:raise SystemExit(f"Node oracle failed: {case['id']}")
                expected=result['stdout'];expected_sha=result['stdout_sha256'];row['expected_stdout']=expected;row['expected_sha256']=expected_sha
            outcome['status']='execution-timeout' if result.get('timeout') else 'runtime-error' if result['exit_code']!=0 else 'pass' if result['stdout']==expected and result['stdout_sha256']==expected_sha else 'wrong-output'
        row['runtimes'][runtime]=outcome
        print(f"compat {case['id']} {runtime}: {outcome['status']}",flush=True)
    report['compatibility'].append(row)

prepared=[]
for name,kind in programs:
    if args.only and name!=args.only:continue
    path=f'benchmarks/programs/{name}.ts'
    row={'id':name,'kind':kind,'source_sha256':sha(ROOT/path),'runtimes':{}}
    commands={}
    for runtime in selected:
        outcome,command=build(runtime,path,name)
        row['runtimes'][runtime]=outcome
        if command:commands[runtime]=command
        print(f"build {name} {runtime}: {outcome['status']}",flush=True)
    prepared.append((row,commands))
# All builds are finished before any accepted timing sample begins.
rng=random.Random(42)
for row,commands in prepared:
    oracle=invoke(commands['node'])
    if oracle['exit_code']!=0:raise SystemExit(f"Node workload failed: {row['id']}")
    expected=oracle['stdout'];expected_sha=oracle['stdout_sha256']
    if row['kind']=='steady':expected=json.loads(expected)['checks']
    raw={runtime:[] for runtime in commands}
    for repetition in range(args.passes):
        order=list(commands);rng.shuffle(order)
        for runtime in order:
            result=invoke(commands[runtime],pin=True)
            sample={'pass':repetition,**result}
            if result['exit_code']!=0:sample['status']='execution-timeout' if result.get('timeout') else 'runtime-error'
            elif row['kind']=='cli':sample['status']='pass' if result['stdout']==expected and result['stdout_sha256']==expected_sha else 'wrong-output'
            else:
                try:
                    payload=json.loads(result['stdout']);sample['payload']=payload
                    valid=payload['checks']==expected and payload['iterations']%16==0 and payload['iterations']>0 and payload['checksum']==(sum(expected)*(payload['iterations']//16))%2**32
                    sample['status']='pass' if valid and payload['elapsedMs']>=100 else 'wrong-output' if not valid else 'timing-too-short'
                    if sample['status']=='pass':sample['ms_per_call']=payload['elapsedMs']/payload['iterations']
                except (ValueError,KeyError,TypeError):sample['status']='invalid-measurement'
            raw[runtime].append(sample)
    for runtime,samples in raw.items():
        outcome=row['runtimes'][runtime];outcome['samples']=samples
        if any(s['status']!='pass' for s in samples):outcome['status']=next(s['status'] for s in samples if s['status']!='pass')
        else:
            values=[s['wall_ms'] if row['kind']=='cli' else s['ms_per_call'] for s in samples]
            outcome.update(status='pass',median_ms=statistics.median(values),min_ms=min(values),max_ms=max(values))
        print(f"measured {row['id']} {runtime}: {outcome['status']} {outcome.get('median_ms','')} ms",flush=True)
    report['benchmarks'].append(row)
report['coverage']={'compat_discovered':len(manifest),'compat_selected':len(report['compatibility']),'compat_deferred':len(manifest)-len(report['compatibility']),'benchmark_discovered':len(programs),'benchmark_selected':len(report['benchmarks']),'benchmark_deferred':len(programs)-len(report['benchmarks'])}
report['machine']['load_after']=list(os.getloadavg())
# Public reports retain reproducible commands without exposing checkout-specific user paths.
serialized=json.dumps(report,indent=2)
for name,binary in tools.items():
    if binary:serialized=serialized.replace(binary,'$'+{'kiln':'KILN_BIN','perry':'PERRY_BIN','porffor':'PORF_BIN','static-hermes':'SHERMES_BIN','node':'NODE_BIN','bun':'BUN_BIN','esbuild':'ESBUILD_BIN'}[name])
serialized=serialized.replace(str(ROOT),'$SITE').replace(str(Path.home()),'$HOME')
Path(args.out).write_text(serialized+'\n')
print('Saved complete evidence to',args.out,flush=True)
