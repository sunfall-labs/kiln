import bisect, collections, gzip, hashlib, json, subprocess
from pathlib import Path
out=Path(__file__).resolve().parent
profile=json.loads(gzip.decompress((out/'profile.json.gz').read_bytes()))
binary=out/'ClusterWorkflowEngine.symbols.native'
rows=[]
for line in subprocess.check_output(['nm','-S','-n','--defined-only',str(binary)],text=True).splitlines():
    p=line.split()
    if len(p)<4 or p[2] not in ('t','T','w','W'): continue
    try: start,size=int(p[0],16),int(p[1],16)
    except ValueError: continue
    if size: rows.append((start,start+size,p[3]))
rows.sort(); starts=[r[0] for r in rows]
def sym_for(frame):
    addr=frame['address'][frame_idx]
    if addr is None or addr<0: return None
    i=bisect.bisect_right(starts,addr)-1
    return rows[i][2] if i>=0 and addr<rows[i][1] else None
def classify(n):
    if not n: return None
    l=n.lower()
    if any(x in l for x in ('gc_', 'cycle_gc','gc_census','weak_ref','weak_collection','trace_edge','candidate_record_pointer_store','realm_gc')): return 'GC tracing/lookup'
    if any(x in l for x in ('malloc','calloc','realloc','free','gc_alloc','allocate','new_object','new_array','object_reserve','array_reserve','shape_new')): return 'allocation'
    if any(x in l for x in ('boundary','to_host','from_host','boundary_value')): return 'boundary conversion'
    if any(x in l for x in ('property','get_own','own_key','key_hash','matches_classified','lookup_key','has_property','key_index','fixed_key','shape_edge','shape_property','string_equal','intern_key','same_name_key')): return 'property lookup/hash'
    if any(x in l for x in ('invoke','call_','_call','dispatch','method','closure_body')): return 'method/call dispatch'
    if n.startswith('kiln_module_path_') and '_fn_' in n: return 'generated body'
    return None
# one sampled thread is the app; sample range begins immediately after case 35 and ends at case 36's timeout.
thread=next(t for t in profile['threads'] if t['processName']=='ClusterWorkflowEngine.native')
ft=thread['frameTable']; st=thread['stackTable']; sm=thread['samples']; funcs=thread['funcTable']; resources=thread['resourceTable']
leaf=collections.Counter(); leaf_cpu=collections.Counter(); group=collections.Counter(); group_cpu=collections.Counter(); top_by_group=collections.defaultdict(collections.Counter); unknown=collections.Counter()
for ix,stack_id in enumerate(sm['stack']):
    if stack_id is None: continue
    weight=sm['threadCPUDelta'][ix] or 0
    sid=stack_id; chain=[]; seen=set()
    while sid is not None:
        frame_idx=st['frame'][sid]
        name=sym_for(ft)
        if name:
            chain.append(name)
            if name not in seen: seen.add(name)
        sid=st['prefix'][sid]
    leafname=chain[0] if chain else '[unresolved]'
    leaf[leafname]+=1; leaf_cpu[leafname]+=weight
    chosen=None
    for name in chain:
        group_name=classify(name)
        if group_name:
            chosen=group_name; top_by_group[group_name][name]+=weight; break
    if chosen is None: chosen='unclassified'; unknown[leafname]+=weight
    group[chosen]+=1; group_cpu[chosen]+=weight
result={'profile':'attached to exact accepted binary during one timeout case; source profile recorded 878 samples over 8.70 seconds; `threadCPUDelta` is profiler CPU interval weight in microseconds, not wall time','binary_sha256':json.loads((out/'run.json').read_text())['binary_sha256'],'symbolized_binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'text_rodata_identity':json.loads((out/'section-identity-profiled.json').read_text())['sections'],'samples':len(sm['stack']),'cpu_delta_us':sum(sm['threadCPUDelta']),'exclusive_groups_by_samples':group.most_common(),'exclusive_groups_by_cpu_us':group_cpu.most_common(),'leaf_top_by_cpu_us':leaf_cpu.most_common(30),'leaf_top_by_samples':leaf.most_common(30),'top_ancestor_by_group_cpu_us':{k:v.most_common(20) for k,v in top_by_group.items()},'unclassified_leaf_cpu_us':unknown.most_common(20)}
(out/'attribution.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
