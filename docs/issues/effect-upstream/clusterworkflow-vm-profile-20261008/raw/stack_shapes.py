import collections,gzip,json,subprocess,sys
prof,binary=sys.argv[1:]
d=json.load(gzip.open(prof,'rt')); t=next(x for x in d['threads'] if x.get('name','').startswith('kiln-77d') and x['samples']['length']>1000); s=t['samples']; st=t['stackTable']; fr=t['frameTable']; ft=t['funcTable']; rt=t['resourceTable']; strings=t['stringArray']
def rsrc(f):
 ri=ft['resource'][fr['func'][f]]
 return d['libs'][rt['lib'][ri]] if ri is not None else None
def rawname(f):
 x=ft['name'][fr['func'][f]]
 return strings[x] if isinstance(x,int) else str(x)
chains=[]; used=set(); shapes=collections.Counter(); maxc=[]
for stack in s['stack']:
 if stack is None: continue
 c=[]; cur=stack
 while cur is not None:
  f=st['frame'][cur]; c.append(f); used.add(f); cur=st['prefix'][cur]
 if len(c)>len(maxc):maxc=c
 chains.append(c)
# symbolize all frames appearing in the single longest stack, and frames from the 15 most common stack ids
stackcounts=collections.Counter(s['stack']); most=[k for k,n in stackcounts.most_common(15) if k is not None]
focus=set(maxc)
for stack in most:
 cur=stack
 while cur is not None:
  focus.add(st['frame'][cur]); cur=st['prefix'][cur]
byimg=collections.defaultdict(list)
for f in focus:
 r=rsrc(f)
 if r and r['path'].startswith('/'): byimg[r['path']].append(f)
sym={}
for path,fs in byimg.items():
 p=subprocess.run(['/usr/bin/llvm-symbolizer','--demangle',f'--obj={path}'],input='\n'.join(hex(fr['address'][f]) for f in fs)+'\n',text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 blocks=[b.strip().splitlines() for b in p.stdout.strip().split('\n\n')]
 for i,f in enumerate(fs): sym[f]=blocks[i] if i<len(blocks) else ['??']
print('total samples',s['length'],'most frequent exact stack frequencies',[(n,k) for k,n in stackcounts.most_common(15)])
print('longest stack depth',len(maxc))
print('LONGEST ROOT-TO-LEAF:')
for f in reversed(maxc): print((rsrc(f) or {}).get('name'),hex(fr['address'][f]),sym.get(f,['??'])[0], (sym.get(f,['??'])[1] if len(sym.get(f,['??']))>1 else ''))
print('TOP exact stack symbols:')
for stack in most[:8]:
 c=[];cur=stack
 while cur is not None:c.append(st['frame'][cur]);cur=st['prefix'][cur]
 print('\n-- samples',stackcounts[stack], 'depth',len(c))
 for f in reversed(c):
  print((rsrc(f) or {}).get('name'),sym.get(f,[rawname(f)])[0])
# count occurrences of resume in fully-symbolized observed shapes
res=0; dup=0; maxdup=0; bystack=[]
for stack,n in stackcounts.items():
 if stack is None:continue
 c=[];cur=stack
 while cur is not None:c.append(st['frame'][cur]);cur=st['prefix'][cur]
 names=[sym.get(f,[rawname(f)])[0] for f in c]
 cnt=sum('resume_generator' in x for x in names)
 if cnt:res+=n;maxdup=max(maxdup,cnt);bystack.append((n,cnt,len(c)))
print('resume-symbol observations among focused frames: sampled_stack_weight',res,'max duplicate frames per stack',maxdup,'stack pattern examples',bystack[:12])
