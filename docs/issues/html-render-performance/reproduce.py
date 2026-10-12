#!/usr/bin/env python3
"""Build HTML-render ablations and compare byte-for-byte answers before timing."""
import argparse, hashlib, json, os, random, statistics, subprocess, time
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('--kiln', required=True, type=Path)
p.add_argument('--work', required=True, type=Path)
p.add_argument('--cpu', type=int, default=15)
p.add_argument('--passes', type=int, default=9)
p.add_argument('--perry-binary', type=Path)
p.add_argument('--current-binary', type=Path)
p.add_argument('--optimized-binary', type=Path)
a = p.parse_args()
a.work = a.work.resolve(); a.work.mkdir(parents=True, exist_ok=True)
a.kiln = a.kiln.resolve()
source = Path(__file__).with_name('template-render.ts').read_text()
join = source.replace('let html = "<table>";', 'const chunks = ["<table>"];')
join = join.replace('html += `<tr>', 'chunks.push(`<tr>').replace('</td></tr>`;', '</td></tr>`);')
join = join.replace('html += "</table>";', 'chunks.push("</table>");\nconst html = chunks.join("");')
manual = '''function esc(s: string): string {
  let out = "";
  for (let i=0; i<s.length; i++) {
    const c = s.charCodeAt(i);
    if(c===38) out += "&amp;";
    else if(c===60) out += "&lt;";
    else if(c===62) out += "&gt;";
    else if(c===34) out += "&quot;";
    else out += s.charAt(i);
  }
  return out;
}
'''
def escape_loop(s):
    return s[:s.index('function esc(')] + manual + s[s.index('\nfunction fnv1a'):]
def replace_all(s):
    for pattern, needle in [('/&/g','"&"'),('/</g','"<"'),('/>/g','">"'),('/"/g',"'\"'")]:
        s = s.replace('.replace('+pattern+',', '.replaceAll('+needle+',')
    return s
hoist = 'const amp = /&/g; const lt = /</g; const gt = />/g; const quote = /"/g;\n' + source
for pattern, name in [('/&/g','amp'),('/</g','lt'),('/>/g','gt'),('/"/g','quote')]:
    hoist = hoist.replace('.replace('+pattern+',', '.replace('+name+',')
variants = {'original':source,'join':join,'hoist':hoist,'manual':escape_loop(source),
            'manual-join':escape_loop(join),'replace-all':replace_all(source),
            'replace-all-join':replace_all(join),
            '1000':source.replace('i < 2000','i < 1000'),
            '4000':source.replace('i < 2000','i < 4000')}
# Reproduce the website's npm-project profile, which provisions kiln's Node layer.
(a.work/'package.json').write_text('{"private":true,"type":"module"}\n')
env = dict(os.environ, KILN_JOBS='2', RAYON_NUM_THREADS='2', TZ='UTC')
commands = {}; expected = {}; hashes = {}
for name, text in variants.items():
    src = a.work/(name+'.ts'); src.write_text(text)
    exe = a.work/(name+'.bin')
    build = subprocess.run([str(a.kiln),'--one-shot','build',str(src),'--profile','native-release','--output',str(exe)], env=env, capture_output=True, timeout=600)
    (a.work/(name+'.build.log')).write_bytes(build.stdout+build.stderr)
    build.check_returncode()
    commands[name] = [str(exe)]
    expected[name] = subprocess.check_output(['node',str(src)],env=env,timeout=60)
    hashes[name] = hashlib.sha256(src.read_bytes()).hexdigest()
for name in variants:
    if name not in ['1000','4000']: assert expected[name] == expected['original'], name
for name, exe in [('perry',a.perry_binary),('current-main',a.current_binary),
                  ('optimized',a.optimized_binary)]:
    if exe: commands[name]=[str(exe.resolve())]; expected[name]=expected['original']
random.seed(42); samples = {name:[] for name in commands}
for _ in range(a.passes):
    order=list(commands); random.shuffle(order)
    for name in order:
        t=time.perf_counter()
        run=subprocess.run(['taskset','-c',str(a.cpu),*commands[name]],env=env,capture_output=True,timeout=60,check=True)
        elapsed=(time.perf_counter()-t)*1000
        assert run.stdout==expected[name], (name, 'wrong output')
        samples[name].append(elapsed)
report = {'cpu':a.cpu,'passes':a.passes,'source_sha256':hashes,
          'kiln_sha256':hashlib.sha256(a.kiln.read_bytes()).hexdigest(),
          'results':{name:{'median_ms':statistics.median(v),'min_ms':min(v),'max_ms':max(v),'samples_ms':v,'stdout_sha256':hashlib.sha256(expected[name]).hexdigest(),'binary_sha256':hashlib.sha256(Path(commands[name][0]).read_bytes()).hexdigest()} for name,v in samples.items()}}
(a.work/'results.json').write_text(json.dumps(report,indent=2)+'\n')
for name,v in report['results'].items():print(f"{name}: {v['median_ms']:.2f} ms, correct output")
