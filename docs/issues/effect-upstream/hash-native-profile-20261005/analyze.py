import bisect,collections,gzip,json
from pathlib import Path
out=Path(__file__).resolve().parent
profile=json.loads(gzip.decompress((out/'profile.json.gz').read_bytes()))
sidecar=json.loads((out/'profile-symbolized.syms.json').read_text())
symbols=sidecar['string_table']
by_name={d['debug_name']:(d['symbol_table'],[s['rva'] for s in d['symbol_table']]) for d in sidecar['data']}
markers=[json.loads(line) for line in (out/'markers.jsonl').read_text().splitlines()]
windows=[];previous=None
for row in markers:
    moment=row['monotonic_ns']/1e6
    if previous is not None and row['record'].get('kind')=='case':
        windows.append((previous,moment,row['record']['id'],row['record']['outcome']))
    previous=moment
leaf=collections.Counter();inclusive=collections.Counter();failed_leaf=collections.Counter();failed_inclusive=collections.Counter();leaf_offsets=collections.Counter();failed_offsets=collections.Counter();sample_count=failed_count=0
for thread in profile['threads']:
    if thread.get('processName')!='native.bin':continue
    frame=thread['frameTable'];func=thread['funcTable'];resource=thread['resourceTable'];stack=thread['stackTable']
    def describe(fi):
        fun=frame['func'][fi];r=func['resource'][fun]
        if r<0:return '[unknown]',None
        libid=resource['lib'][r]
        if libid is None:return '[unknown]',None
        lib=profile['libs'][libid]['debugName'];address=frame['address'][fi]
        if address is None or lib not in by_name:return lib+':unknown',None
        table,starts=by_name[lib];i=bisect.bisect_right(starts,address)-1
        if i>=0 and address<table[i]['rva']+table[i]['size']:
            sym=table[i];return symbols[sym['symbol']],(symbols[sym['symbol']],address-sym['rva'])
        return lib+':unknown',None
    for sid,moment in zip(thread['samples']['stack'],thread['samples']['time']):
        if sid is None:continue
        sample_count+=1
        failing=any(a<=moment<=b and outcome=='fail' for a,b,_,outcome in windows)
        if failing:failed_count+=1
        name,offset=describe(stack['frame'][sid]);leaf[name]+=1
        if offset:leaf_offsets[offset]+=1
        if failing:
            failed_leaf[name]+=1
            if offset:failed_offsets[offset]+=1
        seen=set()
        while sid is not None:
            name,_=describe(stack['frame'][sid]);seen.add(name);sid=stack['prefix'][sid]
        inclusive.update(seen)
        if failing:failed_inclusive.update(seen)
result={'sample_scope':'all native.bin threads; 100Hz sampled CPU counts, not wall-time causality','marker_window_limits':'monotonic milliseconds align with observed raw Linux sample times; intervals include hooks/setup/cleanup and preceding asynchronous activity','samples':sample_count,'failed_window_samples':failed_count,'case_windows':[{'id':id,'outcome':outcome,'wall_s':(b-a)/1000} for a,b,id,outcome in windows],'leaf':leaf.most_common(30),'inclusive':inclusive.most_common(30),'failed_leaf':failed_leaf.most_common(30),'failed_inclusive':failed_inclusive.most_common(30),'failed_offsets':[[name,hex(offset),count] for (name,offset),count in failed_offsets.most_common(30)]}
(out/'attribution.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps({k:result[k] for k in ('samples','failed_window_samples','failed_leaf','failed_inclusive')},indent=2))
