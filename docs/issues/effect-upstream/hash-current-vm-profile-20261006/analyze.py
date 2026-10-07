import bisect, collections, gzip, json
from pathlib import Path
out=Path(__file__).resolve().parent
markers=[json.loads(l) for l in (out/'target-markers.jsonl').open()]
prior=next(x for x in markers if x['record'].get('id')=='Hash > hash law > hashes HashMaps and HashSets independently of insertion order')
failure=next(x for x in markers if x['record'].get('id')=='Hash > hash law > hashes a HashMap like a fresh build after persistent and in-place edits')
start=failure['monotonic_ns']/1e9-(failure['elapsed_s']-prior['elapsed_s'])
end=failure['monotonic_ns']/1e9
D=json.load(gzip.open(out/'profile.json.gz','rt'))
S=json.load((out/'profile.json.syms.json').open())
strs=S['string_table']; maps=[]
for md in S['data']:
 table=md['symbol_table']; starts=[s['rva'] for s in table]; maps.append((table,starts))
def symbol(lib,addr):
 if lib>=len(maps): return f'addr:{addr:x}'
 table,starts=maps[lib]; i=bisect.bisect_right(starts,addr)-1
 if i<0:return f'addr:{addr:x}'
 x=table[i]
 if not x['rva']<=addr<x['rva']+x.get('size',0): return f'addr:{addr:x}'
 return S['data'][lib]['debug_name'] + '::' + strs[x['symbol']]
t=max((t for t in D['threads'] if t['name'].startswith('kiln-cdae5')), key=lambda t:len(t['samples']['time']))
sa=t['samples']; ft=t['frameTable']; st=t['stackTable']; fn=t['funcTable']; rt=t['resourceTable']
rows=[]
for j,(tm,stack) in enumerate(zip(sa['time'],sa['stack'])):
 if start*1000<=tm<=end*1000:
  chain=[]; cur=stack
  while cur is not None:
   frame=st['frame'][cur]; fi=ft['func'][frame]; resource=fn['resource'][fi]; lib=rt['lib'][resource] if resource is not None and 0<=resource<len(rt['lib']) else 0
   addr=ft['address'][frame]
   chain.append(symbol(lib,addr)); cur=st['prefix'][cur]
  rows.append((tm,chain))
leaf=collections.Counter(); incl=collections.Counter()
for _,chain in rows:
 if chain: leaf[chain[0]]+=1
 incl.update(set(chain))
print(json.dumps({'profile':str(out/'profile.json.gz'),'symbols':str(out/'profile.json.syms.json'),'binary_sha256':json.load((out/'target-run-status.json').open())['binary_sha256'],'interval_start_monotonic_s':start,'interval_end_monotonic_s':end,'interval_elapsed_from_case_start_s':failure['elapsed_s']-prior['elapsed_s'],'sample_count':len(rows),'leaf_top':leaf.most_common(25),'inclusive_sample_top':incl.most_common(35),'representative_stacks':[{'time':tm,'symbols':chain[:20]} for tm,chain in rows[:4]+rows[-4:]]},indent=2))
