#!/usr/bin/env python3
"""Render shared-suite results, pairwise performance and recommendations from raw evidence."""
import html
import json
import re
from collections import Counter
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
DATA='runtime-comparison-2026-10-12.json'
record=json.loads((ROOT/'data'/DATA).read_text())
runtimes=['node','bun','kiln','perry','porffor','static-hermes']
labels={'node':'Node','bun':'Bun','kiln':'kiln native','perry':'Perry','porffor':'Porffor','static-hermes':'Static Hermes native'}
names={'startup':'Print-and-exit startup','json-transform':'5,000-row table transform','template-render':'2,000-row HTML render','warm-numeric':'Warmed numeric kernel','warm-json':'Warmed JSON pipeline'}
manifest=json.loads((ROOT/'benchmarks/compat/manifest.json').read_text())
assert {c['id'] for c in record['compatibility']}=={c['id'] for c in manifest}
assert record['coverage']['compat_selected']==len(manifest) and record['coverage']['compat_deferred']==0
assert len(record['benchmarks'])==5 and record['coverage']['benchmark_deferred']==0
assert all(set(c['runtimes'])==set(runtimes) for c in record['compatibility']+record['benchmarks'])
counts=Counter(c['group'] for c in record['compatibility'])

def generated(markup,name,fragment):
    start,end=f'<!-- generated:{name} -->',f'<!-- /generated:{name} -->'
    block=start+fragment+end
    if start in markup:return re.sub(re.escape(start)+'.*?'+re.escape(end),lambda _:block,markup,flags=re.S)
    return markup.replace('</main>',block+'</main>')

def esc(value):return html.escape(str(value))
def table(headers,rows):
    return '<div class="table-scroll" tabindex="0" role="region" aria-label="Shared runtime results"><table><thead><tr>'+''.join(f'<th scope="col">{esc(h)}</th>' for h in headers)+'</tr></thead><tbody>'+''.join('<tr><th scope="row">'+esc(row[0])+'</th>'+''.join(f'<td>{esc(v)}</td>' for v in row[1:])+'</tr>' for row in rows)+'</tbody></table></div>'
def number(value):return f'{value:.4f}' if value<1 else f'{value:.2f}'
def score(runtime,group=None):return sum(c['runtimes'][runtime]['status']=='pass' for c in record['compatibility'] if group is None or c['group']==group)
def result(outcome,kind):
    if outcome['status']!='pass':return outcome['status']
    return number(outcome['median_ms'])+(' ms/call' if kind=='steady' else ' ms')
def ratio(a,b):
    if a['status']!='pass' or b['status']!='pass':return 'No valid pairing'
    x=a['median_ms']/b['median_ms']
    if 1/1.10<=x<=1.10:return 'Medians within 10%'
    return (f'{x:.2f}× lower time for peer' if x>1 else f'{1/x:.2f}× lower time for kiln')

summary=table(['Runtime','Language / '+str(counts['language']),'Host / '+str(counts['host']),'Bundled npm / '+str(counts['npm']),'All selected / '+str(len(manifest))],[(labels[r],score(r,'language'),score(r,'host'),score(r,'npm'),score(r)) for r in runtimes])
matrix=table(['Probe','Group',*[labels[r] for r in runtimes]],[(c['id'],c['group'],*[c['runtimes'][r]['status'] for r in runtimes]) for c in record['compatibility']])
versions=table(['Runtime','Version / revision','Tested profile'],[(labels[r],record['toolchains'][r].get('release') or record['toolchains'][r].get('release_record',{}).get('tag') or record['toolchains'][r].get('source_commit') or record['toolchains'][r]['version'],record['method']['profiles'].get(r,'Direct source execution')) for r in runtimes])
charts=[]
for workload in record['benchmarks']:
    values=[(labels[r],workload['runtimes'][r]['median_ms']) for r in runtimes if workload['runtimes'][r]['status']=='pass']
    maximum=max((v for _,v in values),default=1)*1.1
    height=90+45*len(values);unit='ms / call' if workload['kind']=='steady' else 'ms / process'
    body=[f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 620 {height}" role="img" aria-labelledby="title desc"><title id="title">{esc(names[workload["id"]])}</title><desc id="desc">Lower is better. Zero baseline. '+esc('; '.join(f'{label}: {number(value)} {unit}' for label,value in values))+'</desc>']
    for i,(label,value) in enumerate(values):
        y=15+45*i;body += [f'<text x="0" y="{y+17}" fill="#292d24" font-family="sans-serif" font-size="13">{esc(label)}</text>',f'<rect x="170" y="{y}" width="290" height="24" fill="#ebece3"/>',f'<rect x="170" y="{y}" width="{value/maximum*290:.4f}" height="24" fill="'+('#b6462b' if label=='kiln native' else '#73796b')+'"/>',f'<text x="610" y="{y+17}" text-anchor="end" fill="#292d24" font-family="monospace" font-size="13">{number(value)}</text>']
    y=height-30;body += [f'<path d="M170 {y-10}H460" stroke="#a9ad9f"/>',f'<text x="170" y="{y+8}" fill="#73796b" font-family="monospace" font-size="11">0</text>',f'<text x="460" y="{y+8}" text-anchor="end" fill="#73796b" font-family="monospace" font-size="11">{number(maximum)} {unit}</text>','</svg>']
    chart=f'assets/charts/runtime-{workload["id"]}.svg';(ROOT/chart).write_text('\n'.join(body)+'\n')
    measured=table(['Runtime','Median','Min–max / same unit','Outcome'],[(labels[r],result(workload['runtimes'][r],workload['kind']),f"{number(workload['runtimes'][r]['min_ms'])}–{number(workload['runtimes'][r]['max_ms'])}" if workload['runtimes'][r]['status']=='pass' else '—',workload['runtimes'][r]['status']) for r in runtimes])
    charts.append(f'<h2>{names[workload["id"]]}</h2><p>{unit}; lower is better. '+('Includes process spawn, startup and work; excludes compilation.' if workload['kind']=='cli' else 'Excludes setup, startup and warmup; 200 warmup calls and calibrated batches of at least 100 ms.')+f'</p><img class="diagram-svg reference-chart" src="../{chart}" width="620" height="{height}" alt="Median valid timings; every value and excluded outcome is listed below.">{measured}<p><a download href="../benchmarks/programs/{workload["id"]}.ts">Download the TypeScript workload</a></p>')

common_note=f'<p class="small-note">Same fixtures, frozen toolchains · {record["date"][:10]} · {esc(record["machine"]["cpu"])} · Linux x86_64 · {record["method"]["passes"]} interleaved runs · timing pinned to logical CPU {record["machine"]["timing_cpu"]}. <a href="benchmarks.html">All outcomes, samples and methodology →</a></p>'
for peer,path in [('perry','kiln-vs-perry.html'),('static-hermes','kiln-vs-static-hermes.html'),('porffor','kiln-vs-porffor.html')]:
    peer_label=labels[peer]
    pair_table=table(['Workload','kiln native',peer_label,'Median comparison'],[(names[w['id']],result(w['runtimes']['kiln'],w['kind']),result(w['runtimes'][peer],w['kind']),ratio(w['runtimes']['kiln'],w['runtimes'][peer])) for w in record['benchmarks']])
    failed=[c['id'] for c in record['compatibility'] if c['runtimes']['kiln']['status']=='pass' and c['runtimes'][peer]['status']!='pass']
    kiln_lost=[c['id'] for c in record['compatibility'] if c['runtimes'][peer]['status']=='pass' and c['runtimes']['kiln']['status']!='pass']
    wins={peer:[], 'kiln':[]}
    for w in record['benchmarks']:
        a,b=w['runtimes']['kiln'],w['runtimes'][peer]
        if a['status']=='pass' and b['status']=='pass':
            if a['median_ms']>b['median_ms']*1.10:wins[peer].append(names[w['id']].lower())
            elif b['median_ms']>a['median_ms']*1.10:wins['kiln'].append(names[w['id']].lower())
    recommendations=[]
    if wins[peer]:recommendations.append(f'<p><strong>For {esc(", ".join(wins[peer]))}, evaluate {peer_label} first for performance:</strong> it has the lower measured median on those workloads in this run.</p>')
    if wins['kiln']:recommendations.append(f'<p><strong>For {esc(", ".join(wins["kiln"]))}, evaluate kiln first for performance:</strong> it has the lower measured median on those workloads in this run.</p>')
    if failed:recommendations.append(f'<p><strong>Prefer kiln when you require the behavior exercised by these probes:</strong> {esc(", ".join(failed))}. kiln matched Node; {peer_label} did not complete them with the expected answer in this suite. The full matrix distinguishes compilation failures, runtime failures and wrong output.</p>')
    if kiln_lost:recommendations.append(f'<p><strong>Prefer {peer_label} for the behavior exercised by:</strong> {esc(", ".join(kiln_lost))}. Those probes passed there and failed under the tested kiln release.</p>')
    fragment=f'<section id="measured-comparison"><h2>Matched compatibility and native performance.</h2>{common_note}<p>kiln passed <strong>{score("kiln")} / {len(manifest)}</strong> selected probes; {peer_label} passed <strong>{score(peer)} / {len(manifest)}</strong>. Language, host and bundled-package probes remain separate in the <a href="benchmarks.html#compatibility">complete compatibility matrix</a>. This is a small shared suite, not an overall language or npm compatibility percentage.</p>{pair_table}<h2>Recommendations from this run.</h2>'+''.join(recommendations)+'<p>The recommendations apply to the measured work and tested API behavior. A wrong-answer workload has no performance score. Medians within 10% are treated as close for these recommendations; raw ranges remain visible. CLI and warmed timings answer different questions; only compare values within a row.</p></section>'
    p=ROOT/'compare'/path;s=p.read_text()
    new_title=f'kiln vs {peer_label.removesuffix(" native")}: Native Benchmarks and Compatibility — kiln'
    s=re.sub(r'<title>.*?</title>',lambda _:f'<title>{esc(new_title)}</title>',s)
    description=f'Matched kiln vs {peer_label.removesuffix(" native")} native benchmarks and 24 shared compatibility probes: startup, warmed performance, npm API behavior and raw evidence.'
    s=re.sub(r'<meta name="description" content="[^"]*">',lambda _:f'<meta name="description" content="{esc(description)}">',s)
    s=generated(s,'runtime-comparison',fragment)
    # The editorial renderer states what was available before this measurement; update those sentences.
    replacements={
      'No matched kiln-versus-Perry timing or binary-size run is published here.':'Matched timing results and compatibility outcomes are below; this run does not establish a deployment-size ranking.',
      'There is no matched native-AOT performance result for this pairing on this site.':'The measured native-AOT pairing is below and uses shermes-generated native executables.',
      'No matched current kiln-versus-Porffor performance result is published here.':'The current matched compatibility and timing results are below.',
      'No matched Static Hermes native-AOT results on this site.':'Shared-suite native-AOT results below.',
      'The existing kiln harness lane executes Hermes bytecode, not native AOT.':'The older repository lane runs HBC; this page’s new suite runs shermes native executables.',
    }
    for before,after in replacements.items():s=s.replace(before,after)
    p.write_text(s)

leaders=table(['Workload','Lowest valid median','Lowest native median'],[(names[w['id']],labels[min((r for r in runtimes if w['runtimes'][r]['status']=='pass'),key=lambda r:w['runtimes'][r]['median_ms'])],labels[min((r for r in runtimes if r not in ('node','bun') and w['runtimes'][r]['status']=='pass'),key=lambda r:w['runtimes'][r]['median_ms'])]) for w in record['benchmarks']])
body=f'''<section><h2>Shared tests. Correct answers before speed.</h2><p>All six runtimes receive the same 24 selected probes and five workloads. Node is the oracle. Every compile failure, process failure, timeout or wrong answer remains visible. Every accepted timing sample has its output or every timed return checked against Node.</p>{common_note}<h2 id="compatibility">Compatibility by tested surface</h2>{summary}<p>Counts are passing probes, not supported language features or complete package suites. The bundled npm probes exercise semver, lodash.get, Zod and Effect using exact versions from the lockfile. Package bundling is identical across runtimes; package resolution is not tested by those four rows.</p><h2>Tested toolchains and profiles</h2>{versions}<p>Static Hermes executes native code generated by shermes; its Linux binary links shared Hermes libraries. Perry uses the release’s prebuilt full standard library without the V8 fallback flag. Therefore executable file sizes alone are not a fair deployment-size ranking. TypeScript annotations are stripped with esbuild for Static Hermes; other portable programs use their original TypeScript source. The fixtures are in an npm project, so kiln automatically provisions its Node layer. Standalone-directory startup and optional Static Hermes typed-mode optimization are different profiles from this run.</p><h2>Fastest valid medians in this run</h2>{leaders}<p>These leaders are workload-specific. Wrong outputs are excluded, and startup is distinct from warmed work.</p><h2>How to use these results</h2><p>For a short-lived CLI, compare whole-process timing. For repeated work inside a running service, compare the warmed rows. For an existing application, first identify required language and host behavior in the compatibility matrix. A faster executable with a wrong answer is excluded.</p><p>These are seven samples on one machine, not a statistical significance claim. Warmed results use a real millisecond clock and batches of at least 100 ms. Workloads are single-core and do not benchmark parallel collection methods, actor throughput, GUI rendering or HTTP serving. The <a href="../#measured">separate HTTP benchmark</a> covers Node, Bun and Deno.</p>{''.join(charts)}<h2>Every selected compatibility probe</h2><p class="small-note comparison-hint">Scroll sideways to inspect all six runtime columns.</p>{matrix}<h2>Reproduce and inspect</h2><p>The public evidence contains raw samples, stdout and diagnostics, command lines, binary hashes, source hashes, selected/deferred coverage, package versions and host load. Builds finish before timing, and runtime order is shuffled reproducibly across repetitions.</p><pre><code>npm ci --prefix benchmarks --ignore-scripts\n# Set KILN_BIN, PERRY_BIN, PORF_BIN, SHERMES_BIN and ESBUILD_BIN.\npython3 scripts/compare-runtimes.py \\\n  --out data/{DATA} \\\n  --work .scratch/comparison-run --passes 7 --cpu 15</code></pre><p class="evidence-links"><a href="../data/{DATA}" download>Download complete result JSON</a><a href="../scripts/compare-runtimes.py" download>Download runner</a><a href="../benchmarks/README.md">Profiles and reproduction instructions</a><a href="../benchmarks/compat/manifest.json" download>Every selected probe</a><a href="./">All runtime comparisons</a></p></section>'''
# Use the same site shell without importing the editorial renderer and overwriting its pages.
markup=(ROOT/'compare/index.html').read_text()
start=markup.index('<main id="main"');end=markup.index('</main>',start)+len('</main>')
markup=markup[:start]+'<main id="main" class="reference"><div class="reference-heading"><span class="eyebrow">MEASURED / SHARED RUNTIME SUITE</span><h1>Native runtime benchmarks and compatibility.</h1><p>Matched kiln, Perry, Porffor and Static Hermes native results, with Node and Bun references. Every answer checked before timing.</p></div>'+body+'</main>'+markup[end:]
markup=markup.replace('<title>Compare native TypeScript and JavaScript runtimes — kiln</title>','<title>Native TypeScript Benchmarks and Compatibility — kiln</title>')
# SEO is reapplied after rendering; give the benchmark page its own description now.
import re
markup=re.sub(r'<meta name="description" content="[^"]*">','<meta name="description" content="Matched native runtime benchmarks and 24 shared compatibility probes for kiln, Perry, Porffor and Static Hermes, with Node/Bun references and raw evidence.">',markup)
(ROOT/'compare/benchmarks.html').write_text(markup)
for path in [ROOT/'compare/index.html',ROOT/'index.html']:
    s=path.read_text()
    if path.parent.name=='compare':
        s=s.replace('There are no matched performance results for these three pairings on this site.','<a href="benchmarks.html">The shared native suite</a> now records matched results for all three pairings, with Node and Bun references.')
        s=generated(s,'runtime-hub','<section><h2>Performance and compatibility, measured.</h2><p><a href="benchmarks.html">Inspect the five workloads, 24 shared probes and every raw sample →</a></p></section>')
    else:
        if 'href="compare/benchmarks.html"' not in s:s=s.replace('<a href="compare/">All runtime comparisons →</a>','<a href="compare/">All runtime comparisons →</a><a href="compare/benchmarks.html">Shared benchmarks and compatibility matrix →</a>')
    path.write_text(s)
print('Rendered shared-suite charts, compatibility matrix and pairwise recommendations.')
