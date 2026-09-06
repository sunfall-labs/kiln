import re, sys, collections
xml=open(sys.argv[1]).read()
binname=sys.argv[2]; topn=int(sys.argv[3]) if len(sys.argv)>3 else 25
frames={}; bts={}; binaries={}
sym=collections.Counter(); addrs=collections.Counter()
def attrs(s): return dict(re.findall(r'(\w[\w-]*)="([^"]*)"', s))
for m in re.finditer(r'<row>(.*?)</row>', xml, re.S):
    row=m.group(1)
    bt=re.search(r'<backtrace ([^>]*)>', row)
    if not bt: continue
    a=attrs(bt.group(1))
    if 'ref' in a:
        leaf=bts.get(a['ref'])
    else:
        inner=row[bt.end():]
        fm=re.search(r'<frame ([^>]*?)(/>|>)', inner)
        if not fm: continue
        fa=attrs(fm.group(1))
        if 'ref' in fa:
            leaf=frames.get(fa['ref'])
        else:
            rest=inner[fm.end():]
            bm=re.search(r'<binary ([^>]*)/>', rest)
            b=('?',0)
            if bm:
                ba=attrs(bm.group(1))
                if 'ref' in ba: b=binaries.get(ba['ref'],('?',0))
                else:
                    b=(ba.get('name','?'), int(ba.get('load-addr','0x0'),16)); binaries[ba.get('id','')]=b
            leaf=(fa.get('name','?'), int(fa.get('addr','0x0'),16), b)
            frames[fa.get('id','')]=leaf
        bts[a.get('id','')]=leaf
    if not leaf: continue
    name,addr,b=leaf
    sym[name]+=1
    if b[0]==binname: addrs[(name, addr-b[1])]+=1
tot=sum(sym.values()); print("samples:", tot)
for n,c in sym.most_common(topn): print(f"{c:6d} {n[:90]}")
print("--- hottest instructions (offset from load address)")
for (n,off),c in addrs.most_common(topn): print(f"{c:5d} +0x{off:x}  {n[-24:]}")
