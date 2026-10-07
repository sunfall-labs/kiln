from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import os, shutil, subprocess
cli='~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/date-native-milestone/target/debug/kiln'
root=Path('/var/tmp/kiln')
prefix=f'kiln-cli-extensions-off-diag-{os.getpid()}'

def check(name, files, runs, marked):
    d=root/f'{prefix}-{name}'
    d.mkdir()
    try:
        if marked: (d/'package.json').write_text('{"type":"module"}\n')
        for file,source in files.items(): (d/file).write_text(source)
        results=[]
        for entry,ext in runs:
            env=os.environ.copy(); env['KILN_JOBS']='2'
            if ext is False: env['KILN_EXTENSIONS']='0'
            else: env.pop('KILN_EXTENSIONS',None)
            p=subprocess.run([cli,'--one-shot','--node','run',entry],cwd=d,env=env,text=True,capture_output=True)
            results.append((entry,ext,p.returncode,p.stdout,p.stderr))
        # Keep this fixture until its test thread is done, matching Fixture::drop.
        return name,results
    finally:
        shutil.rmtree(d,ignore_errors=True)

def scenarios(marked):
  plan=[
   ('dynamic',{'dynamic.mjs':"for (const load of [() => import('kiln:shared'), () => import('kiln:actors'), () => import('kiln:process')]) { try { await load(); console.log('imported'); } catch (error) { console.log(error.name, error.message); } }\n"},[('dynamic.mjs',False)]),
   ('static',{'static-shared.mjs':"import 'kiln:shared';\n",'static-actors.mjs':"import 'kiln:actors';\n",'static-process.mjs':"import 'kiln:process';\n"},[(f'static-{s}.mjs',False) for s in ['shared','actors','process']]),
   ('internal',{'core.mjs':"import { isolate } from 'kiln:actors/core';\n",'worker.mjs':"import 'kiln:actors/worker';\n"},[(e,x) for x in [True,False] for e in ['core.mjs','worker.mjs']]),
   ('private',{'private.mjs':"import 'node:worker_threads';\nconsole.log('__kilnPrivate' in globalThis);\n"},[(e,x) for x in [True,False] for e in ['private.mjs']]),
  ]
  with ThreadPoolExecutor(max_workers=4) as pool:
    return list(pool.map(lambda item: check(*item,marked),plan))
for marked in (False,True):
  print('MARKED',marked)
  for name,results in scenarios(marked):
    for entry,ext,code,out,err in results:
      bad=('no such file' in err or 'cannot read' in err)
      print(name,entry,ext,'exit',code,'missing-file',bad,'stderr=',err.strip().replace('\n',' | '))
