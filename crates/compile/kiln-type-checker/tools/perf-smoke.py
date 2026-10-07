#!/usr/bin/env python3
"""Reproducible TypeScript source-check smoke for a frozen kiln release binary."""
from __future__ import annotations
import atexit, argparse, hashlib, json, os, platform, statistics, subprocess, time, uuid
from pathlib import Path

DEFAULT_BINARY = Path('~/Work/kiln-checker-port/target/release/kiln')
DEFAULT_TSC = Path('~/.cache/kiln/type-engine-investigation/typescript-native-7.0.2/package/lib/tsc')
DEFAULT_PROJECT = Path.home()/'.cache/kiln/typechecker-perf/checker-project-v2'


def digest(data: bytes) -> str: return hashlib.sha256(data).hexdigest()
def tree_digest(root: Path) -> tuple[str,int,int]:
    h=hashlib.sha256(); count=total=0
    for p in sorted(x for x in root.rglob('*') if x.is_file() and '.kiln' not in x.relative_to(root).parts):
        rel=p.relative_to(root).as_posix().encode(); body=p.read_bytes()
        h.update(len(rel).to_bytes(4,'big')); h.update(rel); h.update(len(body).to_bytes(8,'big')); h.update(body)
        count+=1; total+=len(body)
    return h.hexdigest(),count,total


def make_project(root: Path, modules: int) -> None:
    src=root/'src'; src.mkdir(parents=True,exist_ok=True)
    (root/'package.json').write_text('{"name":"checker-perf-fixture","private":true,"type":"module"}\n')
    for i in range(modules):
        (src/f'module{i:03}.ts').write_text(
            f'''export interface Metric{i:03} {{ value: number; label: string; ordinal: number; }}\n'''
            f'''export function make{i:03}(value: number): Metric{i:03} {{\n'''
            f'''  return {{ value, label: "metric-{i}", ordinal: {i} }};\n}}\n'''
            f'''export type MaybeMetric{i:03} = Metric{i:03} | undefined;\n'''
            f'''export function read{i:03}(record: MaybeMetric{i:03}): number {{\n'''
            f'''  if (record === undefined) return 0;\n  return record.value + record.ordinal;\n}}\n'''
        )
    imports='\n'.join(f'import {{ make{i:03}, read{i:03} }} from "./module{i:03}.ts";' for i in range(modules))
    uses='\n'.join(f'const row{i:03} = make{i:03}({i});\nconst result{i:03}: number = read{i:03}(row{i:03});' for i in range(modules))
    total=' + '.join(f'result{i:03}' for i in range(modules))
    (src/'main.ts').write_text(imports+'\n\n'+uses+f'\nexport const total = {total};\n')
    config={"compilerOptions":{"target":"ESNext","module":"Preserve","moduleResolution":"Bundler","strict":True,"noUncheckedIndexedAccess":True,"allowImportingTsExtensions":True,"noEmit":True,"skipLibCheck":True,"lib":["esnext"]},"include":["src/**/*.ts"]}
    (root/'tsconfig.json').write_text(json.dumps(config,indent=2)+'\n')


def fork_run(argv: list[str], cwd: Path, env: dict[str,str], outpath: Path) -> dict:
    outpath.parent.mkdir(parents=True,exist_ok=True)
    fd=os.open(outpath,os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o644)
    pid=os.fork()
    if pid==0:
        try:
            os.dup2(fd,1); os.dup2(fd,2); os.close(fd); os.chdir(cwd)
            os.execve(argv[0],argv,env)
        except BaseException as e:
            os.write(2,f'exec failure: {e}\n'.encode()); os._exit(127)
    os.close(fd); start=time.perf_counter(); _,status,usage=os.wait4(pid,0); elapsed=time.perf_counter()-start
    code=os.waitstatus_to_exitcode(status)
    rss_kib=usage.ru_maxrss//1024 if platform.system()=='Darwin' else usage.ru_maxrss
    return {"argv":argv,"exit_code":code,"wall_ms":round(elapsed*1000,3),"max_rss_kib":rss_kib,"output":str(outpath)}


def stats(rows: list[dict]) -> dict:
    vals=[x['wall_ms'] for x in rows]
    return {"n":len(vals),"median_ms":round(statistics.median(vals),3),"min_ms":round(min(vals),3),"max_ms":round(max(vals),3),"median_child_max_rss_kib":int(statistics.median(x['max_rss_kib'] for x in rows))}


def proc_for_socket(socket: str) -> int|None:
    out=subprocess.run(['ps','-eo','pid=,args='],text=True,stdout=subprocess.PIPE,check=True).stdout
    for line in out.splitlines():
        if 'kiln daemon serve' in line and socket in line:
            try: return int(line.strip().split(None,1)[0])
            except (ValueError,IndexError): pass
    return None


def rss_status(pid: int|None) -> dict|None:
    if pid is None: return None
    try: text=Path(f'/proc/{pid}/status').read_text()
    except OSError: return None
    found={}
    for line in text.splitlines():
        if line.startswith(('VmRSS:','VmHWM:')):
            key,val=line.split(':',1); found[key]=val.strip()
    return {"pid":pid,**found}


def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('--kiln',type=Path,default=DEFAULT_BINARY); ap.add_argument('--tsc',type=Path,default=DEFAULT_TSC)
    ap.add_argument('--project',type=Path,default=DEFAULT_PROJECT,help='durable parent directory for a fresh generated project')
    ap.add_argument('--modules',type=int,default=80); ap.add_argument('--runs',type=int,default=9); args=ap.parse_args()
    if args.modules<1: ap.error('--modules must be positive')
    if args.runs<2: ap.error('--runs must be at least 2')
    root=args.project.expanduser().resolve()/f"run-{uuid.uuid4().hex[:12]}"
    root.mkdir(parents=True,exist_ok=False)
    make_project(root,args.modules)
    expected_files={Path('package.json'),Path('tsconfig.json'),Path('src/main.ts')}|{Path(f'src/module{i:03}.ts') for i in range(args.modules)}
    actual_files={p.relative_to(root) for p in root.rglob('*') if p.is_file()}
    if actual_files!=expected_files: raise SystemExit(f"generated fixture inventory differs: missing={expected_files-actual_files} extra={actual_files-expected_files}")
    try: (root/'.kiln/source-index').unlink()
    except FileNotFoundError: pass
    source_hash,file_count,byte_count=tree_digest(root)
    if file_count!=len(expected_files): raise SystemExit(f"fixture digest included generated or stale files: expected {len(expected_files)}, found {file_count}")
    kiln=args.kiln.resolve(); tsc=args.tsc.resolve()
    version=subprocess.run([str(kiln),'--version'],text=True,capture_output=True,check=True).stdout.strip()
    tsc_version=subprocess.run([str(tsc),'--version'],text=True,capture_output=True,check=True).stdout.strip()
    if 'protocol' not in version or tsc_version!='Version 7.0.2': raise SystemExit(f'unexpected compiler versions: {version!r} / {tsc_version!r}')
    env=os.environ.copy(); env['LC_ALL']='C'; env['KILN_JOBS']='8'; env['RAYON_NUM_THREADS']='8'
    out=Path('.scratch/checker-port')/f"perf-smoke-{time.strftime('%Y%m%d')}.json"
    rawdir=out.with_suffix(''); rawdir=rawdir.parent/(rawdir.name+'-raw'); rawdir.mkdir(parents=True,exist_ok=True)
    records={"one_shot":[],"daemon_start":None,"daemon_warmup":[],"daemon_warm":[],"daemon_edited":[],"tsc":[]}
    for i in range(args.runs):
        local=env.copy(); local['KILN_SOCKET']=f'/var/tmp/kiln-checker-perf-one-{os.getpid()}-{i}.sock'
        records['one_shot'].append(fork_run([str(kiln),'--one-shot','check','src/main.ts'],root,local,rawdir/f'kiln-one-{i:02}.log'))
    socket=f'/var/tmp/kiln-checker-perf-{os.getpid()}-{uuid.uuid4().hex[:10]}.sock'
    local=env.copy(); local['KILN_SOCKET']=socket
    owned={'pid':None}
    def cleanup_daemon():
        if owned['pid'] is not None:
            try: subprocess.run([str(kiln),'daemon','stop'],cwd=root,env=local,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=10)
            except Exception: pass
    atexit.register(cleanup_daemon)
    records['daemon_start']=fork_run([str(kiln),'check','src/main.ts'],root,local,rawdir/'kiln-daemon-start.log')
    daemon_pid=proc_for_socket(socket)
    owned['pid']=daemon_pid
    records['daemon_pid']=daemon_pid
    for i in range(3): records['daemon_warmup'].append(fork_run([str(kiln),'check','src/main.ts'],root,local,rawdir/f'kiln-warmup-{i:02}.log'))
    for i in range(args.runs): records['daemon_warm'].append(fork_run([str(kiln),'check','src/main.ts'],root,local,rawdir/f'kiln-warm-{i:02}.log'))
    records['daemon_rss_after']=rss_status(daemon_pid)
    for i in range(args.runs): records['tsc'].append(fork_run([str(tsc),'--pretty','false','--noEmit','--project','tsconfig.json'],root,env,rawdir/f'tsc-{i:02}.log'))
    main_module=root/'src/module000.ts'; clean_module=main_module.read_text()
    invalid_module=clean_module+'\nexport const checker_bench_invalid: number = "not-a-number";\n'
    main_module.write_text(invalid_module)
    records['invalid_fixture_sha256']=digest(invalid_module.encode())
    for i in range(args.runs):
        if i:
            main_module.write_text(invalid_module+f'// edit revision {i}\n')
        row=fork_run([str(kiln),'check','src/main.ts'],root,local,rawdir/f'kiln-edited-{i:02}.log')
        row['source_sha256']=digest(main_module.read_bytes())
        records['daemon_edited'].append(row)
    records['tsc_negative']=fork_run([str(tsc),'--pretty','false','--noEmit','--project','tsconfig.json'],root,env,rawdir/'tsc-negative.log')
    main_module.write_text(clean_module)
    stop=fork_run([str(kiln),'daemon','stop'],root,local,rawdir/'kiln-daemon-stop.log')
    owned['pid']=None
    records['daemon_stop']=stop
    errors=[]
    for group in ('one_shot','daemon_warm','tsc'):
        for row in records[group]:
            if row['exit_code']!=0: errors.append(f"{group} exit {row['exit_code']} in {row['output']}")
    for row in records['daemon_edited']:
        if row['exit_code']!=1 or 'TS2322' not in Path(row['output']).read_text(errors='replace'):
            errors.append(f"edited check did not report expected TS2322 in {row['output']}")
    negative=Path(records['tsc_negative']['output']).read_text(errors='replace')
    if records['tsc_negative']['exit_code']==0 or 'error TS2322' not in negative:
        errors.append('pinned tsc did not report expected negative-fixture TS2322')
    for group in ('daemon_start','daemon_warmup'):
        rows=[records[group]] if group=='daemon_start' else records[group]
        if any(row['exit_code']!=0 for row in rows): errors.append(f"{group} had a nonzero exit")
    for row in records['one_shot']+records['daemon_warm']+[records['daemon_start']]+records['tsc']:
        output=Path(row['output']).read_text(errors='replace')
        if 'error TS' in output: errors.append(f"unexpected diagnostic output in {row['output']}")
    if stop['exit_code']!=0: errors.append(f"daemon stop failed: {stop['output']}")
    repo=Path(__file__).resolve().parents[4]
    commit=subprocess.run(['git','-C',str(repo),'rev-parse','HEAD'],text=True,capture_output=True).stdout.strip()
    binary_stat=kiln.stat(); oracle_stat=tsc.stat()
    report={"schema":1,"commit":commit,"binary":{"path":str(kiln),"version":version,"mtime_ns":binary_stat.st_mtime_ns,"size":binary_stat.st_size,"sha256":digest(kiln.read_bytes())},"oracle":{"path":str(tsc),"version":tsc_version,"mtime_ns":oracle_stat.st_mtime_ns,"size":oracle_stat.st_size,"sha256":digest(tsc.read_bytes())},"fixture":{"path":str(root),"module_count":args.modules,"files":file_count,"bytes":byte_count,"sha256":source_hash,"edited_module_sha256":records['invalid_fixture_sha256'],"config":"strict, noUncheckedIndexedAccess, ESNext, Preserve, Bundler, noEmit"},"environment":{"platform":platform.platform(),"cpu_count":os.cpu_count(),"kiln_jobs":env['KILN_JOBS'],"rayon_threads":env['RAYON_NUM_THREADS']},"measurements":{"one_shot_processes":stats(records['one_shot']),"one_shot_first_ms":records['one_shot'][0]['wall_ms'],"one_shot_repeated_processes":stats(records['one_shot'][1:]),"daemon_start_ms":records['daemon_start']['wall_ms'],"daemon_warmup":stats(records['daemon_warmup']),"daemon_warm_requests":stats(records['daemon_warm']),"daemon_edited_requests":stats(records['daemon_edited']),"typescript_cli_fresh_processes":stats(records['tsc'])},"comparison_notes":["Warm daemon timings measure cached no-change client/IPC checks and are not comparable to a fresh tsc invocation.","One-shot and tsc both start fresh processes, but compiler implementations and diagnostics/metadata behavior differ; no speedup ratio is asserted."],"records":records,"errors":errors,"raw_directory":str(rawdir.resolve())}
    out.parent.mkdir(parents=True,exist_ok=True); out.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps({k:report[k] for k in ('commit','binary','oracle','fixture','environment','measurements','errors','raw_directory')},indent=2))
    if errors: raise SystemExit(1)

if __name__=='__main__': main()
