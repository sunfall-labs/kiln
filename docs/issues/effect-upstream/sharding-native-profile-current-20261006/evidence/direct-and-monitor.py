import json, os, pathlib, subprocess, sys, threading, time
binary, cwd, log_path, pid_path, monitor_path, status_path = map(pathlib.Path, sys.argv[1:])
start = time.monotonic(); wall = time.time()
proc = subprocess.Popen([str(binary)], cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, bufsize=0)
pid_path.write_text(json.dumps({'pid':proc.pid,'binary':str(binary),'started_unix_s':wall,'started_monotonic_s':start,'proc_exe':os.readlink(f'/proc/{proc.pid}/exe')})+'\n')
stop = threading.Event()
def stat_for(path):
    try:
        line = path.read_text(); rparen = line.rfind(')')
        fields = line[rparen+2:].split()
        return {'state':fields[0], 'utime_ticks':int(fields[11]), 'stime_ticks':int(fields[12]), 'comm':line[line.find('(')+1:rparen]}
    except (OSError, ValueError, IndexError): return None

def monitor():
    with monitor_path.open('w') as out:
        out.write('mono_s\ttid\tstate\tutime_ticks\tstime_ticks\tcomm\n'); out.flush()
        while not stop.is_set():
            now = time.monotonic()-start
            taskdir = pathlib.Path(f'/proc/{proc.pid}/task')
            try: tasks = list(taskdir.iterdir())
            except OSError: tasks = []
            for p in tasks:
                row=stat_for(p/'stat')
                if row: out.write(f"{now:.6f}\t{p.name}\t{row['state']}\t{row['utime_ticks']}\t{row['stime_ticks']}\t{row['comm']}\n")
            out.flush(); stop.wait(.1)
th = threading.Thread(target=monitor, daemon=True); th.start()
with log_path.open('wb') as out:
    for b in iter(proc.stdout.readline, b''):
        out.write(f'{time.monotonic()-start:.6f}\t'.encode()+b); out.flush()
code=proc.wait(); stop.set(); th.join()
status={'binary':str(binary),'pid':proc.pid,'proc_exe':os.readlink(f'/proc/{proc.pid}/exe') if pathlib.Path(f'/proc/{proc.pid}').exists() else 'exited','started_unix_s':wall,'started_monotonic_s':start,'duration_s':time.monotonic()-start,'exit_code':code}
status_path.write_text(json.dumps(status,indent=2)+'\n'); raise SystemExit(code)
