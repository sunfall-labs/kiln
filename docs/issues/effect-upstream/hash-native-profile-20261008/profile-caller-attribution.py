import bisect,collections,gzip,json,re,subprocess
from pathlib import Path
out=Path(__file__).resolve().parent
p=json.loads(gzip.decompress((out/'profile.json.gz').read_bytes()))
markers=[json.loads(s) for s in (out/'markers.jsonl').read_text().splitlines()]
failed='Hash > hash law > hashes a HashMap like a fresh build after persistent and in-place edits'
prev=None; window=None
for row in markers:
 t=row['monotonic_ns']/1e6
 if prev is not None and row['record'].get('kind')=='case' and row['record'].get('id')==failed: window=(prev,t)
 prev=t
bin=out/'native-symbolized.bin'
section=next(x for x in subprocess.check_output(['readelf','-W','-S',str(bin)],text=True).splitlines() if re.search(r'\s\.text\s',x)).split()
start=int(section[3],16); end=start+int(section[5],16)
syms=[]
for line in subprocess.check_output(['nm','-S','-n','--defined-only',str(bin)],text=True,errors='replace').splitlines():
 m=re.match(r'^([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+([a-zA-Z])\s+(\S+)$',line)
 if m:
  a,z,typ,name=m.groups();a=int(a,16);z=int(z,16)
  if typ.lower()=='t' and start<=a<end:syms.append((a,z,name))
syms.sort(); starts=[s[0] for s in syms]
def describe(thread,fi):
 f=thread['frameTable'];fn=f['func'][fi];r=thread['funcTable']['resource'][fn]
 if r<0:return '[unknown]'
 libid=thread['resourceTable']['lib'][r]
 if libid is None:return '[unknown]'
 lib=p['libs'][libid]['debugName'];addr=f['address'][fi]
 if lib!='native.bin' or addr is None:return lib+':unknown'
 i=bisect.bisect_right(starts,addr)-1
 if i<0:return 'native.bin:unknown'
 a,z,n=syms[i]; lim=a+z if z else (syms[i+1][0] if i+1<len(syms) else a)
 return n if a<=addr<lim else 'native.bin:unknown'
TARGETS=['kiln_c99_new_object_sized','kiln_val_to_boundary','kiln_val_from_boundary','kiln_c99_own_index_key_with_hash','kiln_c99_gc_lookup_nursery','kiln_c99_gc_trace','kiln_c99_gc_finalize','kiln_c99_intern_key','kiln_c99_get_own_kind']
counts={t:collections.Counter() for t in TARGETS}; examples={t:collections.Counter() for t in TARGETS}; leaves=collections.Counter()
for th in p['threads']:
 if th.get('processName')!='native.bin':continue
 frames=th['frameTable'];st=th['stackTable']
 for sid,tm in zip(th['samples']['stack'],th['samples']['time']):
  if sid is None or not(window[0]<=tm<=window[1]):continue
  chain=[];q=sid
  while q is not None:
   chain.append(describe(th,st['frame'][q]));q=st['prefix'][q]
  for target in TARGETS:
   if target not in chain:continue
   counts[target]['inclusive_sample']=1
   ti=chain.index(target); caller=chain[ti+1] if ti+1<len(chain) else '[root]'
   app=next((n for n in chain[ti+1:] if n.startswith('kiln_module_path_') and '_fn_' in n),None)
   generic=next((n for n in chain[ti+1:] if n.startswith('ku_fn_')),None)
   counts[target]['leaf_sample']+=int(chain[0]==target)
   examples[target][(caller,app or '[none]',generic or '[none]',tuple(chain[max(0,ti-2):ti+9]))]+=1
res={'window_ms':list(window),'sample_count':sum(1 for t in p['threads'] if t.get('processName')=='native.bin' for sid,tm in zip(t['samples']['stack'],t['samples']['time']) if sid is not None and window[0]<=tm<=window[1]),'targets':{}}
for t in TARGETS:
 res['targets'][t]={'sample_counts':dict(counts[t]),'caller_groups':[{'direct_parent':k[0],'nearest_module_function':k[1],'nearest_generic_function':k[2],'count':v,'local_stack':list(k[3])} for k,v in examples[t].most_common(30)]}
print(json.dumps(res,indent=2))
(out/'caller-attribution.json').write_text(json.dumps(res,indent=2)+'\n')
