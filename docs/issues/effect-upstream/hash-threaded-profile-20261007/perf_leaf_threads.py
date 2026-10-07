#!/usr/bin/env python3
"""Bounded Linux task-clock sampler with one perf ring per discovered TID.

Separate diagnostic candidate. It does not modify cluster-leaf-perf/perf_leaf.py.
"""
import argparse
import hashlib
import importlib.util
import json
import mmap
import os
import resource
import select
import signal
import struct
import sys
import time
from pathlib import Path

BASE = Path('~/Work/kiln/.scratch/cluster-leaf-perf/perf_leaf.py')
spec = importlib.util.spec_from_file_location('perf_leaf_base', BASE)
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)
PAGE = base.PAGE
RING_BYTES = PAGE * 2
base.RING_BYTES = RING_BYTES
MAX_THREADS = 24
MAX_SAMPLES = 500_000
MAX_MARKERS = 4096
MAX_MARKER_BYTES = 8192
MAP_REFRESH_NS = 250_000_000
EVENT_CLOCK = 'cpu'
EVENT_MODE = 'frequency'
EVENT_RATE_HZ = 49.0

class StopRequested(Exception):
    pass

def on_stop(_signum, _frame):
    raise StopRequested()

def sha(path):
    h=hashlib.sha256()
    with open(path,'rb') as f:
        for block in iter(lambda:f.read(1<<20),b''): h.update(block)
    return h.hexdigest()

def stat_ticks(tid):
    try:
        raw=Path(f'/proc/{tid}/stat').read_text(); cols=raw[raw.rfind(')')+2:].split()
        return int(cols[11])+int(cols[12])
    except (OSError,ValueError,IndexError): return None

def maps_for(pid):
    maps = base.parse_maps(pid)
    # Preserve the fields needed to distinguish a genuine mapping from an
    # address range that was reused after dlclose/munmap.  The file offset,
    # device and inode together identify a mapped ELF segment; load bias is
    # derived offline from its PT_LOAD entries.
    try:
        for line in Path(f'/proc/{pid}/maps').read_text().splitlines():
            cols = line.split(maxsplit=5)
            if len(cols) < 5:
                continue
            lo = int(cols[0].split('-', 1)[0], 16)
            dev, inode = cols[3], int(cols[4])
            item = next((m for m in maps if m['start'] == lo), None)
            if item is not None:
                item['device'] = dev
                item['inode'] = inode
    except (FileNotFoundError, ProcessLookupError, PermissionError, ValueError):
        pass
    return maps

def parse_cmdline(pid):
    return base.parse_cmdline(pid)

def task_ids(pid):
    try: return sorted(int(p.name) for p in Path(f'/proc/{pid}/task').iterdir())
    except (FileNotFoundError,ProcessLookupError,PermissionError): return []

def open_thread_event(tid, freq, enable_on_exec=False):
    fd=base.perf_open(tid,freq,enable_on_exec=enable_on_exec)
    try:
        mm=mmap.mmap(fd,PAGE+RING_BYTES,flags=mmap.MAP_SHARED,prot=mmap.PROT_READ|mmap.PROT_WRITE,offset=0)
        off=int.from_bytes(mm[1040:1048],'little'); size=int.from_bytes(mm[1048:1056],'little')
        if off!=PAGE or size!=RING_BYTES:
            raise RuntimeError(f'unsupported mmap layout {off}/{size}, expected {PAGE}/{RING_BYTES}')
        base.fcntl_ioctl(fd,base.PERF_EVENT_IOC_RESET)
        if not enable_on_exec: base.fcntl_ioctl(fd,base.PERF_EVENT_IOC_ENABLE)
        return fd,mm
    except BaseException:
        os.close(fd)
        raise

def map_signature(maps):
    return tuple((m['start'],m['end'],m['perms'],m['offset'],m.get('device'),m.get('inode'),m['path']) for m in maps)

def is_executable_ip(maps,ip):
    return any(m['start']<=ip<m['end'] and 'x' in m['perms'] for m in maps)

def drain_event(event, samples, maps_by_pid, maps_snapshots, map_snapshot_by_pid, last_map_refresh, unknown_pages_by_pid, totals):
    head=int.from_bytes(event['mm'][1024:1032],'little')
    pending=max(0, head-event['tail'])
    event['max_pending_bytes']=max(event['max_pending_bytes'],pending)
    pos=event['tail']
    if head-pos<=RING_BYTES:
        while pos<head:
            h=base.ring_copy(event['mm'],pos,8)
            if len(h)!=8: break
            rtype,_misc,size=struct.unpack('<IHH',h)
            if size<8 or size>RING_BYTES: break
            if rtype==5: totals['throttle_records']+=1
            elif rtype==6: totals['unthrottle_records']+=1
            pos+=size
    batch,lost,other,over,tail=base.parse_records(event['mm'],event['tail'],head)
    observed=time.monotonic_ns()
    if batch:
        pid=batch[0]['pid']; key=str(pid); current=maps_by_pid.get(key,[])
        unknown_pages={s['ip'] >> 12 for s in batch if not is_executable_ip(current,s['ip'])}
        unseen_pages=unknown_pages-unknown_pages_by_pid.setdefault(key,set())
        refresh_due=observed-last_map_refresh.get(key,0)>=MAP_REFRESH_NS
        if not current or unseen_pages or refresh_due:
            refreshed=maps_for(pid)
            if refreshed:
                previous_id=map_snapshot_by_pid.get(key)
                previous_sig=map_signature(current) if current else None
                refreshed_sig=map_signature(refreshed)
                if previous_sig!=refreshed_sig or previous_id is None:
                    previous_id=len(maps_snapshots)
                    maps_snapshots.append({'pid':pid,'observed_monotonic_ns':observed,'maps':refreshed})
                    map_snapshot_by_pid[key]=previous_id
                maps_by_pid[key]=refreshed
                current=refreshed
            last_map_refresh[key]=observed
            unknown_pages_by_pid[key].clear()
        unknown_pages_by_pid[key].update(unknown_pages)
        snapshot_id=map_snapshot_by_pid.get(key)
        for s in batch:
            s['observed_monotonic_ns']=observed
            s['maps_snapshot_id']=snapshot_id
    if len(samples)+len(batch)>MAX_SAMPLES:
        totals['sample_limit_exceeded']=True
        batch=batch[:max(0,MAX_SAMPLES-len(samples))]
    samples.extend(batch)
    totals['lost']+=lost; totals['other']+=other; totals['overruns']+=over
    event['tail']=tail
    event['ring_wraps']=tail//RING_BYTES
    # perf writes head and waits for userspace to publish tail before reusing
    # ring storage. Updating only our Python copy eventually stalls sampling.
    struct.pack_into('<Q',event['mm'],1032,tail)
    event['sample_count']+=len(batch)

def consume_lines(pending, markers, totals):
    while b'\n' in pending:
        line,pending=pending.split(b'\n',1)
        if b'KILN_EFFECT ' not in line:
            continue
        if len(line)>MAX_MARKER_BYTES or len(markers)>=MAX_MARKERS:
            totals['marker_limit_exceeded']=True
            continue
        s=line.decode('utf-8','replace'); raw=s.split('KILN_EFFECT ',1)[1]
        try: obj=json.loads(raw)
        except json.JSONDecodeError: obj=None
        markers.append({'arrival_monotonic_ns':time.monotonic_ns(),'kind':(obj or {}).get('kind'),'object':obj,'line':s})
    if len(pending)>MAX_MARKER_BYTES:
        # Preserve raw stdout separately, but always bound the parser's
        # incomplete-line buffer. A marker line crossing the cap invalidates
        # the parsed-marker view; ordinary large lines are simply discarded
        # from this auxiliary parser after their bytes have been logged.
        if b'KILN_EFFECT ' in pending:
            totals['marker_limit_exceeded']=True
        pending=pending[-MAX_MARKER_BYTES:]
    return pending

def run(command, output, stdout_log, focus_exe, expected_sha, freq, address_limit, cpu=None):
    base.require_platform()
    if not command: raise ValueError('command required')
    if os.path.realpath(focus_exe)!=os.path.realpath(command[0]):
        raise ValueError('direct mode requires focus executable == command[0]')
    if cpu is not None and cpu not in os.sched_getaffinity(0): raise ValueError('requested CPU outside affinity')
    target_exe=os.path.realpath(focus_exe)
    pre_sha=sha(target_exe)
    if expected_sha and pre_sha!=expected_sha.lower(): raise ValueError('prelaunch target SHA mismatch')
    gate_r,gate_w=os.pipe(); out_r,out_w=os.pipe()
    pid=os.fork()
    if pid==0:
        try:
            os.close(gate_w); os.close(out_r); os.dup2(out_w,1); os.dup2(out_w,2); os.close(out_w)
            if address_limit: resource.setrlimit(resource.RLIMIT_AS,(address_limit,address_limit))
            if cpu is not None: os.sched_setaffinity(0,{cpu})
            os.read(gate_r,1); os.close(gate_r); os.execvp(command[0],command)
        except BaseException as e:
            os.write(2,(f'thread sampler child setup/exec failed: {e}\n').encode()); os._exit(127)
    os.close(gate_r); os.close(out_w)
    events={}; image_fd=None; stdout=None; reaped=False; waitstatus=None; usage=None
    samples=[]; markers=[]; maps_by_pid={}; maps_snapshots=[]; map_snapshot_by_pid={}; last_map_refresh={}; unknown_pages_by_pid={}; totals={'lost':0,'other':0,'overruns':0,'throttle_records':0,'unthrottle_records':0,'sample_limit_exceeded':False,'marker_limit_exceeded':False}
    stdout_hash=hashlib.sha256(); stdout_bytes=0; eof=False; pending=b''; drain_until=None; attach_errors=[]; cap_exceeded=False
    per_tid=[]; target_start=time.monotonic_ns(); expected_cmd=[os.fsencode(s) for s in command]
    try:
        stdout=open(stdout_log,'xb',buffering=0) if stdout_log else None
        # Attach a leader event before exec; workers are individually attached after discovery.
        fd,mm=open_thread_event(pid,freq,enable_on_exec=True)
        events[pid]={'fd':fd,'mm':mm,'tail':0,'sample_count':0,'attached_ns':time.monotonic_ns(),'ticks_start':None,'ticks_last':None,'ring_wraps':0,'max_pending_bytes':0}
        os.write(gate_w,b'x'); os.close(gate_w); gate_w=None
        deadline=time.monotonic()+2
        exec_seen=False
        while time.monotonic()<deadline:
            maps=maps_for(pid)
            if parse_cmdline(pid)==expected_cmd and any(x['path']==target_exe for x in maps): exec_seen=True; break
            time.sleep(.002)
        if not exec_seen: raise RuntimeError('exec identity could not be confirmed')
        map_key=str(pid); initial_map_id=len(maps_snapshots)
        maps_snapshots.append({'pid':pid,'observed_monotonic_ns':time.monotonic_ns(),'maps':maps})
        maps_by_pid[map_key]=maps; map_snapshot_by_pid[map_key]=initial_map_id
        last_map_refresh[map_key]=time.monotonic_ns()
        image_fd=os.open(f'/proc/{pid}/exe',os.O_RDONLY|os.O_CLOEXEC)
        events[pid]['ticks_start']=stat_ticks(pid)
        os.set_blocking(out_r,False)
        while True:
            now=time.monotonic_ns()
            tids=task_ids(pid)
            for tid in tids:
                if tid in events: continue
                if len(events)>=MAX_THREADS:
                    cap_exceeded=True; continue
                try:
                    tfd,tmm=open_thread_event(tid,freq,False)
                    events[tid]={'fd':tfd,'mm':tmm,'tail':0,'sample_count':0,'attached_ns':now,'ticks_start':stat_ticks(tid),'ticks_last':None,'ring_wraps':0,'max_pending_bytes':0}
                except OSError as e:
                    attach_errors.append({'tid':tid,'errno':e.errno,'error':str(e)})
                except Exception as e:
                    attach_errors.append({'tid':tid,'error':str(e)})
            ready,_,_=select.select([out_r],[],[],0.005)
            if ready:
                try: data=os.read(out_r,65536)
                except BlockingIOError: data=b''
                if data:
                    stdout_hash.update(data); stdout_bytes+=len(data)
                    if stdout: stdout.write(data)
                    pending=consume_lines(pending+data,markers,totals)
                else: eof=True
            for tid,event in list(events.items()):
                drain_event(event,samples,maps_by_pid,maps_snapshots,map_snapshot_by_pid,last_map_refresh,unknown_pages_by_pid,totals)
                ticks=stat_ticks(tid)
                if ticks is not None: event['ticks_last']=ticks
            if not reaped:
                done,status,ru=os.wait4(pid,os.WNOHANG)
                if done:
                    reaped=True; waitstatus=status; usage=ru; drain_until=time.monotonic()+1.5
            if reaped and (eof or time.monotonic()>=drain_until): break
        if pending and b'KILN_EFFECT ' in pending:
            if len(pending)>MAX_MARKER_BYTES or len(markers)>=MAX_MARKERS:
                totals['marker_limit_exceeded']=True
            else:
                s=pending.decode('utf-8','replace'); raw=s.split(b'KILN_EFFECT ',1)[1].decode('utf-8','replace')
                try: obj=json.loads(raw)
                except json.JSONDecodeError: obj=None
                markers.append({'arrival_monotonic_ns':time.monotonic_ns(),'kind':(obj or {}).get('kind'),'object':obj,'line':s})
        if stdout: stdout.flush(); os.fsync(stdout.fileno()); stdout.close(); stdout=None
        per_tid_results=[]; counter_ns=enabled_ns=running_ns=0
        for tid,event in events.items():
            base.fcntl_ioctl(event['fd'],base.PERF_EVENT_IOC_DISABLE)
            drain_event(event,samples,maps_by_pid,maps_snapshots,map_snapshot_by_pid,last_map_refresh,unknown_pages_by_pid,totals)
            counter=base.read_counter(event['fd'])
            counter_ns+=counter['value_ns']; enabled_ns+=counter['time_enabled_ns']; running_ns+=counter['time_running_ns']
            per_tid_results.append({'tid':tid,'attached_monotonic_ns':event['attached_ns'],'sample_count':event['sample_count'],
                                    'ring_wraps':event['ring_wraps'],'tail_final':event['tail'],'max_pending_bytes':event['max_pending_bytes'],
                                    'counter':counter,'ticks_start':event['ticks_start'],'ticks_last':event['ticks_last']})
        identity=base.hash_fd(image_fd); os.close(image_fd); image_fd=None
        elapsed=time.monotonic_ns()-target_start
        child_cpu_ns=int((usage.ru_utime+usage.ru_stime)*1e9) if usage else None
        child_user_cpu_ns=int(usage.ru_utime*1e9) if usage else None
        child_system_cpu_ns=int(usage.ru_stime*1e9) if usage else None
        period_sum=sum(s['period'] for s in samples)
        result={'schema':1,'sampler':'per-thread-event-rings-v1','command':command,'pid':pid,'instrumented_exe':target_exe,
                'prelaunch_sha256':pre_sha,'image':identity,'expected_sha256':expected_sha,
                'expected_sha256_match':identity['sha256']==expected_sha if expected_sha else None,
                'exec_observed':exec_seen,'exit_code':os.waitstatus_to_exitcode(waitstatus) if waitstatus is not None else None,
                'wall_ns':elapsed,'child_rusage_cpu_ns':child_cpu_ns,'child_rusage_user_cpu_ns':child_user_cpu_ns,'child_rusage_system_cpu_ns':child_system_cpu_ns,'counter':{'value_ns':counter_ns,'time_enabled_ns':enabled_ns,'time_running_ns':running_ns},
                'counter_to_rusage_ratio':counter_ns/child_cpu_ns if child_cpu_ns else None,
                'sample_period_sum':period_sum,'sample_period_to_counter_ratio':period_sum/counter_ns if counter_ns else None,
                'sample_density_ratio':len(samples)/(EVENT_RATE_HZ*counter_ns/1e9) if counter_ns else None,
                'sample_count':len(samples),'lost_records':totals['lost'],'other_records':totals['other'],'ring_overruns_bytes':totals['overruns'],
                'throttle_records':totals['throttle_records'],'unthrottle_records':totals['unthrottle_records'],'event_clock':EVENT_CLOCK,'event_mode':EVENT_MODE,'target_rate_hz':EVENT_RATE_HZ,
                'sample_limit_exceeded':totals['sample_limit_exceeded'],'marker_limit_exceeded':totals['marker_limit_exceeded'],'thread_cap_exceeded':cap_exceeded,
                'thread_event_count':len(events),'thread_attach_errors':attach_errors,'per_tid':per_tid_results,
                'sampled_tids':sorted({s['tid'] for s in samples}),'samples':samples,'maps_by_pid':maps_by_pid,'maps_snapshots':maps_snapshots,'stdout_markers':markers,
                'stdout_capture':{'bytes':stdout_bytes,'sha256':stdout_hash.hexdigest(),'eof_observed':eof},
                'sampling_scope':'separate non-inheriting perf event and ring for each discovered task TID; new TIDs polled every 5ms'}
        sample_maps=lambda s: maps_snapshots[s['maps_snapshot_id']]['maps'] if isinstance(s.get('maps_snapshot_id'),int) and s['maps_snapshot_id']<len(maps_snapshots) else maps_by_pid.get(str(s['pid']),[])
        result['sampled_unmapped_ip_count']=sum(not is_executable_ip(sample_maps(s),s['ip']) for s in samples)
        result['focus_sample_count']=sum(any(m['start']<=s['ip']<m['end'] and m['path'].removesuffix(' (deleted)')==target_exe and 'x' in m['perms'] for m in sample_maps(s)) for s in samples)
        result['focus_image_attribution_available']=result['focus_sample_count']>0
        result['user_mode_sample_count']=sum((s['misc']&7)==2 for s in samples)
        return result
    finally:
        if gate_w is not None:
            try: os.close(gate_w)
            except OSError: pass
        try: os.close(out_r)
        except OSError: pass
        if stdout: stdout.close()
        for event in events.values():
            try: event['mm'].close()
            except Exception: pass
            try: os.close(event['fd'])
            except OSError: pass
        if image_fd is not None:
            try: os.close(image_fd)
            except OSError: pass
        if not reaped:
            try: os.kill(pid,signal.SIGKILL)
            except ProcessLookupError: pass
            try: os.waitpid(pid,0)
            except ChildProcessError: pass

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('--focus-exe',required=True); ap.add_argument('--expected-focus-sha256')
    ap.add_argument('--freq',type=int,default=49); ap.add_argument('--address-limit-bytes',type=int,default=8<<30)
    ap.add_argument('--output',required=True); ap.add_argument('--stdout-log'); ap.add_argument('--cpu',type=int)
    ap.add_argument('--event-clock',choices=['cpu','task'],default='cpu')
    ap.add_argument('--event-mode',choices=['frequency','fixed-period'],default='frequency')
    ap.add_argument('--period-ns',type=int,default=20_408_163)
    ap.add_argument('command',nargs=argparse.REMAINDER); a=ap.parse_args()
    cmd=a.command[1:] if a.command[:1]==['--'] else a.command
    if not cmd: ap.error('command required after --')
    if a.freq<1 or a.freq>12000: ap.error('frequency must be 1..12000')
    if a.period_ns<83_334 or a.period_ns>1_000_000_000: ap.error('period-ns must imply 1..12000 samples/s')
    global EVENT_CLOCK,EVENT_MODE,EVENT_RATE_HZ
    EVENT_CLOCK=a.event_clock
    EVENT_MODE=a.event_mode
    EVENT_RATE_HZ=a.freq if a.event_mode=='frequency' else 1_000_000_000/a.period_ns
    base.PERF_COUNT_SW_CPU_CLOCK=0 if a.event_clock=='cpu' else 1
    if a.event_mode=='fixed-period':
        # Software CPU/TASK_CLOCK periods are measured in nanoseconds. With
        # freq=0, leave attr.freq clear so the kernel uses this fixed interval.
        def perf_open_fixed(pid,period_ns,enable_on_exec=True):
            attr=base.PerfEventAttr(); attr.type=base.PERF_TYPE_SOFTWARE; attr.size=base.ctypes.sizeof(attr)
            attr.config=base.PERF_COUNT_SW_CPU_CLOCK; attr.sample_period=period_ns
            attr.sample_type=base.PERF_SAMPLE_IP|base.PERF_SAMPLE_TID|base.PERF_SAMPLE_TIME|base.PERF_SAMPLE_PERIOD
            attr.read_format=base.PERF_FORMAT_TOTAL_TIME_ENABLED|base.PERF_FORMAT_TOTAL_TIME_RUNNING
            attr.flags=(1<<0)|(1<<5)|(1<<6)
            if enable_on_exec: attr.flags|=(1<<12)
            attr.wakeup_events=1
            fd=base.libc.syscall(base.ctypes.c_long(298),base.ctypes.byref(attr),base.ctypes.c_int(pid),base.ctypes.c_int(-1),base.ctypes.c_int(-1),base.ctypes.c_ulong(base.PERF_FLAG_FD_CLOEXEC))
            if fd<0:
                e=base.ctypes.get_errno(); raise OSError(e,f'perf_event_open failed: {os.strerror(e)}')
            return int(fd)
        base.perf_open=perf_open_fixed
    signal.signal(signal.SIGTERM,on_stop); signal.signal(signal.SIGINT,on_stop)
    try:
        event_value=a.period_ns if a.event_mode=='fixed-period' else a.freq
        r=run(cmd,a.output,a.stdout_log,a.focus_exe,a.expected_focus_sha256,event_value,a.address_limit_bytes,a.cpu)
        r['event_parameter']=event_value
        quality=(r['thread_event_count']>=1 and not r['thread_cap_exceeded'] and not r['thread_attach_errors'] and not r['lost_records'] and not r['ring_overruns_bytes'] and not r['throttle_records'] and not r['sample_limit_exceeded'] and not r['marker_limit_exceeded'] and r['stdout_capture']['eof_observed'] and r['sampled_unmapped_ip_count']==0 and r['expected_sha256_match'] is not False and r['counter_to_rusage_ratio'] is not None and 0.8<=r['counter_to_rusage_ratio']<=1.2 and r['sample_period_to_counter_ratio'] is not None and 0.8<=r['sample_period_to_counter_ratio']<=1.2 and r['sample_density_ratio'] is not None and 0.8<=r['sample_density_ratio']<=1.2 and r['user_mode_sample_count']==r['sample_count'])
        r['sampling_quality_ok']=quality
        print(json.dumps({'workload_exit_code':r['exit_code'],'sampling_quality_ok':quality,'sample_count':r['sample_count'],'thread_event_count':r['thread_event_count'],'sampled_tids':r['sampled_tids'],'lost_records':r['lost_records'],'ring_overruns_bytes':r['ring_overruns_bytes'],'marker_limit_exceeded':r['marker_limit_exceeded'],'counter':r['counter'],'child_rusage_cpu_ns':r['child_rusage_cpu_ns'],'counter_to_rusage_ratio':r['counter_to_rusage_ratio'],'sample_density_ratio':r['sample_density_ratio'],'focus_sample_count':r['focus_sample_count']},indent=2))
        Path(a.output).write_text(json.dumps(r,indent=2)+'\n')
        return 0 if quality else 2
    except StopRequested:
        return 143
    except Exception as e:
        print(f'thread sampler error: {type(e).__name__}: {e}',file=sys.stderr)
        raise
if __name__=='__main__': raise SystemExit(main())
