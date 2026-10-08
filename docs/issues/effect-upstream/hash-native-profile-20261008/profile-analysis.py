import bisect, collections, gzip, hashlib, json, re, subprocess
from pathlib import Path
out=Path(__file__).resolve().parent
profile_path=out/'profile.json.gz'; symbin=out/'native-symbolized.bin'; marker_path=out/'markers.jsonl'
profile=json.loads(gzip.decompress(profile_path.read_bytes()))
markers=[json.loads(line) for line in marker_path.read_text().splitlines()]
windows=[]; previous=None
for row in markers:
    moment=row['monotonic_ns']/1e6
    if previous is not None and row['record'].get('kind')=='case':
        windows.append((previous,moment,row['record']['id'],row['record']['outcome']))
    previous=moment
# The diagnostic relink has the same complete .text bytes as profile's stripped binary.
# Map sampled executable RVAs to its retained ELF local symbols.
text=subprocess.check_output(['readelf','-W','-S',str(symbin)],text=True)
textline=next(x for x in text.splitlines() if re.search(r'\s\.text\s',x))
parts=textline.split(); textaddr=int(parts[3],16); textsize=int(parts[5],16)
raw=subprocess.check_output(['nm','-S','-n','--defined-only',str(symbin)],text=True,errors='replace')
symbols=[]
for line in raw.splitlines():
    m=re.match(r'^([0-9a-fA-F]+)\s+([0-9a-fA-F]+)\s+([a-zA-Z])\s+(\S+)$',line)
    if not m: continue
    addr,size,typ,name=m.groups(); addr=int(addr,16); size=int(size,16)
    if typ.lower()=='t' and textaddr<=addr<textaddr+textsize:
        symbols.append((addr,size,name,typ))
symbols.sort()
starts=[s[0] for s in symbols]
def lookup(address, libname):
    if address is None or libname!='native.bin': return f'{libname}:unknown',None
    i=bisect.bisect_right(starts,address)-1
    if i>=0:
        start,size,name,typ=symbols[i]
        end=(start+size) if size else (symbols[i+1][0] if i+1<len(symbols) else start)
        if start<=address<end: return name,(name,address-start)
    return 'native.bin:unknown',None
leaf=collections.Counter(); inclusive=collections.Counter(); failed_leaf=collections.Counter(); failed_inclusive=collections.Counter(); failed_offsets=collections.Counter(); failed_paths=collections.Counter(); sample_count=fail_count=0
thread_reports=[]
for thread in profile['threads']:
    if thread.get('processName')!='native.bin': continue
    frame=thread['frameTable']; func=thread['funcTable']; resource=thread['resourceTable']; stack=thread['stackTable']; tsamples=thread['samples']
    ts=0; fs=0
    def describe(fi):
        fn=frame['func'][fi]; r=func['resource'][fn]
        if r<0: return '[unknown]',None
        libid=resource['lib'][r]
        if libid is None: return '[unknown]',None
        lib=profile['libs'][libid]['debugName']; address=frame['address'][fi]
        return lookup(address,lib)
    for sid,moment in zip(tsamples['stack'],tsamples['time']):
        if sid is None: continue
        sample_count+=1; ts+=1
        fail=any(a<=moment<=b and outc=='fail' for a,b,_,outc in windows)
        if fail: fail_count+=1; fs+=1
        name,offset=describe(stack['frame'][sid]); leaf[name]+=1
        if fail:
            failed_leaf[name]+=1
            if offset: failed_offsets[offset]+=1
        seen=set(); chain=[]
        while sid is not None:
            n,_=describe(stack['frame'][sid]); chain.append(n); seen.add(n); sid=stack['prefix'][sid]
        inclusive.update(seen)
        if fail:
            failed_inclusive.update(seen)
            failed_paths[tuple(chain[:16])]+=1
    thread_reports.append({'tid':thread.get('tid'),'samples':ts,'failed_window_samples':fs,'name':thread.get('name')})
res={'scope':'100 Hz samply CPU samples; leaf=sample leaf frame; inclusive=count of samples whose stack contains symbol (deduplicated per stack); interval is previous emitted case record to failed case record and includes runner hooks/setup/cleanup and any async spillover','profile_process_threads':thread_reports,'samples':sample_count,'failed_window_samples':fail_count,'case_windows':[{'id':i,'outcome':o,'start_ms':a,'end_ms':b,'wall_s':(b-a)/1000} for a,b,i,o in windows],'top_leaf_all':leaf.most_common(50),'top_leaf_failed':failed_leaf.most_common(80),'top_inclusive_failed':failed_inclusive.most_common(100),'top_failed_offsets':[[n,hex(off),c] for (n,off),c in failed_offsets.most_common(40)],'top_failed_paths':[[list(path),count] for path,count in failed_paths.most_common(30)],'text':{'address':textaddr,'size':textsize,'sha256':hashlib.sha256(subprocess.check_output(['objcopy','--dump-section',f'.text=/dev/stdout',str(symbin)])).hexdigest() if False else 'see relink receipt'},'symbol_count':len(symbols)}
(out/'attribution.json').write_text(json.dumps(res,indent=2)+'\n')
print(json.dumps({k:res[k] for k in ('samples','failed_window_samples','profile_process_threads','case_windows','top_leaf_failed','top_inclusive_failed')},indent=2))
