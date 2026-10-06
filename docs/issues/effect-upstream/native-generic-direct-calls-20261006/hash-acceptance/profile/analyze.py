import bisect, collections, gzip, json
from pathlib import Path

root = Path(__file__).resolve().parent
profile = json.loads(gzip.decompress((root / 'hash-window.json.gz').read_bytes()))
sidecar = json.loads((root / 'hash-window.json.syms.json').read_text())
symbols = sidecar['string_table']
by_name = {d['debug_name']: (d['symbol_table'], [s['rva'] for s in d['symbol_table']]) for d in sidecar['data']}
rows = [json.loads(line) for line in (root / 'timestamped-native.log').read_text().splitlines()]
for row in rows:
    row['record'] = json.loads(row['line'].split('KILN_EFFECT ', 1)[1])
fail = next(r for r in rows if r['record'].get('kind') == 'case' and r['record'].get('outcome') == 'fail')
prior = [r for r in rows if r['record'].get('kind') == 'case' and r['unix_s'] < fail['unix_s']]
start = prior[-1]['unix_s']
end = fail['unix_s']
leaf = collections.Counter()
inclusive = collections.Counter()
unknown = 0
sample_n = 0
thread_rows=[]
for thread in profile['threads']:
    if thread.get('name') != 'native-hash-symbols.bin':
        continue
    frame, func, resource, stack = (thread[k] for k in ('frameTable','funcTable','resourceTable','stackTable'))
    samples = thread['samples']
    def describe(fi):
        fun = frame['func'][fi]
        rid = func['resource'][fun]
        if rid < 0 or resource['lib'][rid] is None:
            return '[unknown]'
        lib = profile['libs'][resource['lib'][rid]]['debugName']
        addr = frame['address'][fi]
        if addr is None or lib not in by_name:
            return lib + ':unknown'
        table, starts = by_name[lib]
        i = bisect.bisect_right(starts, addr) - 1
        if i >= 0 and addr < table[i]['rva'] + table[i]['size']:
            return symbols[table[i]['symbol']]
        return lib + ':unknown'
    n=0
    for sid, ms in zip(samples['stack'], samples['time']):
        unix_s = (profile['meta']['startTime'] + ms - samples['time'][0]) / 1000
        if sid is None or not start <= unix_s <= end:
            continue
        n += 1; sample_n += 1
        leaf[describe(stack['frame'][sid])] += 1
        seen=set()
        while sid is not None:
            seen.add(describe(stack['frame'][sid]))
            sid=stack['prefix'][sid]
        inclusive.update(seen)
    thread_rows.append({'name':thread['name'],'pid':thread.get('pid'),'tid':thread.get('tid'),'window_samples':n})

def pct(n): return round(n * 100 / sample_n, 2) if sample_n else 0
result = {
 'scope':'samples whose profile timestamp falls between the prior passing case marker and failing Hash law timeout marker; this includes harness setup/cleanup and preceding asynchronous activity, not an exact isolated callback',
 'case_id':fail['record']['id'], 'window_start_unix_s':start, 'window_end_unix_s':end,
 'window_wall_seconds':round(end-start,6), 'profile_sample_count':sample_n,
 'thread_matches':thread_rows,
 'top_self_samples':[{'symbol':s,'samples':n,'share_pct':pct(n)} for s,n in leaf.most_common(30)],
 'top_inclusive_presence':[{'symbol':s,'samples_containing_symbol':n,'share_pct':pct(n)} for s,n in inclusive.most_common(30)],
}
(root/'analysis.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
