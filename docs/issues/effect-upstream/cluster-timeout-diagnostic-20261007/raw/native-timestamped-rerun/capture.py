import json, os, resource, select, subprocess, sys, time
from pathlib import Path
root=Path(__file__).resolve().parents[1]
out=Path(__file__).resolve().parent
binary=root/'native-stage4/ClusterWorkflowEngine.native'
raw=out/'stdout.log'; arrivals=out/'stdout-arrivals.jsonl'; cpu=out/'cpu-snapshots.jsonl'; status=out/'status.json'; before=out/'ps-before.txt'; after=out/'ps-after.txt'
command=[str(binary)]
env=dict(os.environ, TZ='UTC', KILN_JOBS='1', RAYON_NUM_THREADS='1')
def memlimit():
    cap=8*1024**3
    resource.setrlimit(resource.RLIMIT_AS,(cap,cap))
def stat(pid):
    try:
        s=Path(f'/proc/{pid}/stat').read_text(); rest=s[s.rfind(') ')+2:].split()
        return {'state':rest[0], 'utime_ticks':int(rest[11]),'stime_ticks':int(rest[12])}
    except (FileNotFoundError,ProcessLookupError,ValueError,IndexError): return None
def all_threads(pid):
    try: tids=sorted(int(p.name) for p in Path(f'/proc/{pid}/task').iterdir())
    except FileNotFoundError: return []
    result=[]
    for tid in tids:
        x=stat(f'{pid}/task/{tid}')
        if x is not None:
            try: x['comm']=Path(f'/proc/{pid}/task/{tid}/comm').read_text().strip()
            except OSError: x['comm']='?'
            x['tid']=tid; result.append(x)
    return result
Path(before).write_text(subprocess.run(['ps','-eo','pid,ppid,etime,pcpu,rss,args'],capture_output=True,text=True,check=True).stdout)
start_mono=time.monotonic_ns(); start_wall=time.time_ns()
proc=subprocess.Popen(command,cwd=root,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,preexec_fn=memlimit,bufsize=0)
pid=proc.pid
running={'pid':pid,'exe':os.readlink(f'/proc/{pid}/exe'),'command':command,'binary_sha256':subprocess.check_output(['sha256sum',str(binary)],text=True).split()[0],'start_monotonic_ns':start_mono,'start_unix_ns':start_wall,'initial_stat':stat(pid),'memory_cap_bytes':8*1024**3}
(out/'running.json').write_text(json.dumps(running,indent=2)+'\n')
fd=proc.stdout.fileno(); os.set_blocking(fd,False); buf=b''; idx=0; eof=False; next_cpu=time.monotonic_ns()
with raw.open('wb') as rawf, arrivals.open('w') as arrf, cpu.open('w') as cpuf:
    while not eof:
        ready,_,_=select.select([fd],[],[],0.025)
        now=time.monotonic_ns()
        if ready:
            try: chunk=os.read(fd,65536)
            except BlockingIOError: chunk=b''
            if chunk:
                rawf.write(chunk); rawf.flush(); buf+=chunk
                while b'\n' in buf:
                    line,buf=buf.split(b'\n',1); idx+=1
                    arrf.write(json.dumps({'index':idx,'monotonic_ns':now,'unix_ns':time.time_ns(),'line':line.decode('utf-8','replace')},ensure_ascii=False)+'\n'); arrf.flush()
            elif proc.poll() is not None: eof=True
        elif proc.poll() is not None:
            # Drain remaining buffered bytes before considering EOF.
            try: chunk=os.read(fd,65536)
            except BlockingIOError: chunk=b''
            if chunk:
                rawf.write(chunk); rawf.flush(); buf+=chunk
            else: eof=True
            while b'\n' in buf:
                line,buf=buf.split(b'\n',1); idx+=1
                arrf.write(json.dumps({'index':idx,'monotonic_ns':now,'unix_ns':time.time_ns(),'line':line.decode('utf-8','replace')},ensure_ascii=False)+'\n'); arrf.flush()
        if now>=next_cpu:
            cpuf.write(json.dumps({'monotonic_ns':now,'unix_ns':time.time_ns(),'process':stat(pid),'threads':all_threads(pid)})+'\n'); cpuf.flush(); next_cpu=now+100_000_000
        if now-start_mono>300_000_000_000:
            proc.kill(); eof=True
code=proc.wait(); ended_mono=time.monotonic_ns(); ended_wall=time.time_ns()
if buf:
    idx+=1; line=buf; arrf=None
    with arrivals.open('a') as af: af.write(json.dumps({'index':idx,'monotonic_ns':ended_mono,'unix_ns':ended_wall,'line':line.decode('utf-8','replace')},ensure_ascii=False)+'\n')
Path(after).write_text(subprocess.run(['ps','-eo','pid,ppid,etime,pcpu,rss,args'],capture_output=True,text=True,check=True).stdout)
status.write_text(json.dumps({'exit_code':code,'start_monotonic_ns':start_mono,'end_monotonic_ns':ended_mono,'start_unix_ns':start_wall,'end_unix_ns':ended_wall,'wall_seconds':(ended_mono-start_mono)/1e9,'native_user_ticks':resource.getrusage(resource.RUSAGE_CHILDREN).ru_utime,'native_system_ticks':resource.getrusage(resource.RUSAGE_CHILDREN).ru_stime,'stdout_lines':idx,'stdout_bytes':raw.stat().st_size,'binary_sha256':running['binary_sha256']},indent=2)+'\n')
raise SystemExit(code)
