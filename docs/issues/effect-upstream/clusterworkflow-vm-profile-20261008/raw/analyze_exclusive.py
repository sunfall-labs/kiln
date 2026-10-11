import collections,gzip,json,subprocess,sys
prof,binary,out=sys.argv[1:]; d=json.load(gzip.open(prof,'rt')); t=next(x for x in d['threads'] if x.get('name','').startswith('kiln-77d') and x['samples']['length']>1000); s=t['samples'];st=t['stackTable'];fr=t['frameTable'];ft=t['funcTable'];rt=t['resourceTable'];strings=t['stringArray']
def resource(f):
 ri=ft['resource'][fr['func'][f]]; li=rt['lib'][ri] if ri is not None else None; return d['libs'][li] if li is not None else None
leaf=collections.Counter(); lcpu=collections.Counter(); inclusive=collections.Counter(); allused=set(); stacks=[]
for i,stack in enumerate(s['stack']):
 if stack is None:continue
 chain=[];cur=stack
 while cur is not None:
  f=st['frame'][cur];chain.append(f);allused.add(f);cur=st['prefix'][cur]
 if chain:leaf[chain[0]]+=1;lcpu[chain[0]]+=s['threadCPUDelta'][i]
 inclusive.update(chain);stacks.append((chain,i))
# Resolve every sampled frame address, grouped by exact image, through DWARF symbols.
byimg=collections.defaultdict(list)
for f in allused:
 r=resource(f)
 if r and r['path'].startswith('/'):byimg[r['path']].append(f)
sym={}
for path,fs in byimg.items():
 p=subprocess.run(['/usr/bin/llvm-symbolizer','--demangle',f'--obj={path}'],input='\n'.join(hex(fr['address'][f]) for f in fs)+'\n',text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True)
 blocks=[b.strip().splitlines() for b in p.stdout.strip().split('\n\n')]
 for i,f in enumerate(fs):sym[f]=blocks[i] if i<len(blocks) else ['??']
def symbol(f):return sym.get(f,['??'])[0]
def category(f):
 r=resource(f); image=r['name'] if r else 'unknown'; n=symbol(f)
 if n=='??': return 'unresolved-symbol'
 if image in ('libc.so.6','ld-linux-x86-64.so.2','[vdso]'): return 'system-libc-loader'
 if image.startswith('kiln-'):
  z=n.lower()
  if any(x in z for x in ('alloc::','__rust_alloc','malloc','realloc','drop_glue','clone::clone')): return 'Rust-allocation-clone-drop'
  if any(x in z for x in ('cycle_gc','ownergraph','owner_graph')): return 'VM-cycle-GC-owner-graph'
  if any(x in z for x in ('run_bounded','run_frames','resume_generator','call_value','invoke','dispatch')): return 'VM-instruction-call-generator-dispatch'
  if any(x in z for x in ('object_storage','object_model','propertykey','find_prototype','get_key','is_internal_key')): return 'VM-property-key-lookup'
  return 'other-kiln'
 return 'other-runtime-image'
cc=collections.Counter();ct=collections.Counter()
for f,n in leaf.items():cc[category(f)]+=n;ct[category(f)]+=lcpu[f]
# Identify all leaf's top25 with DSO, resolved symbol, source, and exact exclusive samples.
rows=[]
for f,n in leaf.most_common(25):
 r=resource(f); rows.append({'samples':n,'share_pct':round(100*n/s['length'],3),'cpu_ms':round(lcpu[f]/1e6,3),'image':r['name'] if r else None,'address':hex(fr['address'][f]),'symbol':symbol(f),'source':sym.get(f,['??'])[1:3],'category':category(f)})
# Full chain frequency and recursive evidence. llvm-symbolizer resolves all stack frames used.
sc=collections.Counter(s['stack']); resume_samples=0;max_resume=0;shapes=collections.Counter();maxchain=max(stacks,key=lambda z:len(z[0]))[0]
for stack,n in sc.items():
 if stack is None:continue
 c=[];cur=stack
 while cur is not None:c.append(st['frame'][cur]);cur=st['prefix'][cur]
 names=[symbol(f) for f in c]; count=sum('resume_generator' in x for x in names)
 if count:resume_samples+=n;max_resume=max(max_resume,count)
 # exact frame symbol chain root -> leaf, truncate event-loop root boilerplate but keep VM calls
 roots=[x for x in reversed(names) if any(k in x for k in ('resume_generator','run_bounded','run_frames','call_value_inner','invoke','run_node_event_loop_turns','collect_closure_cycles','OwnerGraph'))]
 shapes[tuple(roots[-12:])]+=n
result={'profile':{'thread':t['name'],'tid':t['tid'],'samples':s['length'],'thread_cpu_ms':round(sum(s['threadCPUDelta'])/1e6,3),'profile_interval_ms':d['meta']['interval'],'kiln_code_id':next(x['codeId'] for x in d['libs'] if x['name'].startswith('kiln-'))},'exclusive_leaf_categories':[{'category':k,'samples':v,'share_pct':round(100*v/s['length'],3),'cpu_ms':round(ct[k]/1e6,3)} for k,v in cc.most_common()],'top25_leaf_frames':rows,'stack_structure':{'max_frames':len(maxchain),'max_resume_generator_frames_in_one_stack':max_resume,'sample_weight_with_resume_generator':resume_samples,'top_vm_call_shapes':[{'samples':n,'frames':list(k)} for k,n in shapes.most_common(12)],'max_stack_vm_frames_root_to_leaf':[symbol(f) for f in reversed(maxchain) if any(k in symbol(f) for k in ('resume_generator','run_bounded','run_frames','call_value_inner','invoke','run_node_event_loop_turns','collect_closure_cycles','OwnerGraph'))]}}
open(out,'w').write(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
