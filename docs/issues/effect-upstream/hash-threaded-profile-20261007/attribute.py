import json,struct,collections,subprocess,hashlib
from pathlib import Path
out=Path(__file__).resolve().parent
r=json.loads((out/'profile.json').read_text());spec=json.loads((out.parent/'capture-spec-threaded-v3.json').read_text());exe=Path(spec['binary']['path'])
with exe.open('rb') as f:
 hdr=f.read(64);phoff=struct.unpack_from('<Q',hdr,32)[0];phsz,phn=struct.unpack_from('<HH',hdr,54);f.seek(phoff);phs=[struct.unpack('<IIQQQQQQ',f.read(phsz)) for _ in range(phn)]
load=[p for p in phs if p[0]==1]
symbols=[]
for line in subprocess.check_output(['readelf','-Ws',str(exe)],text=True).splitlines():
 fields=line.split()
 if len(fields)>=8 and fields[3]=='FUNC' and fields[6]!='UND':
  start,size=int(fields[1],16),int(fields[2],16) if fields[2].startswith('0x') else int(fields[2]);name=fields[7]
  if size:symbols.append((start,start+size,name))
symbols=list(set(symbols));counts=collections.Counter();target=collections.Counter();ambiguous=0;focus=0
deltas=[s['observed_monotonic_ns']-s['perf_time_ns'] for s in r['samples']];offset=sorted(deltas)[len(deltas)//100];spread=max(deltas)-min(deltas);slack=spread+int(1e9/49)
prior=None;window=None
for m in r['stdout_markers']:
 if m['object'].get('id')==spec['baseline_report']['hash_timeout_case']:window=(prior,m['arrival_monotonic_ns']);break
 if m['kind']=='case':prior=m['arrival_monotonic_ns']
for s in r['samples']:
 maps=r['maps_snapshots'][s['maps_snapshot_id']]['maps'];ms=[m for m in maps if m['start']<=s['ip']<m['end'] and 'x' in m['perms']]
 assert len(ms)==1
 m=ms[0]
 if m['path']!=str(exe):continue
 focus+=1;fileoff=s['ip']-m['start']+m['offset'];ps=[p for p in load if p[2]<=fileoff<p[2]+p[5]];assert len(ps)==1
 p=ps[0];addr=fileoff-p[2]+p[3];matches=[sym for sym in symbols if sym[0]<=addr<sym[1]]
 name=matches[0][2] if len(matches)==1 else '<symbol gap>' if not matches else '<ambiguous symbol>'
 ambiguous+=len(matches)>1;counts[name]+=1;aligned=s['perf_time_ns']+offset
 if spread<100000000 and window and window[0]+slack<aligned<window[1]-slack:target[name]+=1
report={'scope':'exclusive VM Hash diagnostic leaves; not acceptance or inclusive stacks','sample_quality':r['sampling_quality_ok'],'official_result':json.loads((out/'official-validation.json').read_text())['records']['summary'],'focus_samples':focus,'ambiguous_symbols':ambiguous,'full_top':counts.most_common(25),'target_interval_top':target.most_common(25),'target_interval_samples':sum(target.values()),'clock_alignment':{'offset_ns':offset,'spread_ns':spread,'slack_ns':slack,'window':window,'per_case_attribution_valid':spread<100000000},'symbolization':'exact ELF STT_FUNC ranges; reject overlapping aliases; no nearest-symbol fallback','profile_sha256':hashlib.sha256((out/'profile.json').read_bytes()).hexdigest()}
(out/'leaf-attribution.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({'focus_samples':focus,'ambiguous':ambiguous,'target_samples':sum(target.values()),'full_top':counts.most_common(3)},indent=2))
