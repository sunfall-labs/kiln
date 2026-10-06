import json,os,subprocess,sys,time
from pathlib import Path
root=Path.cwd();dest=root/'.scratch/array-dispatch-combined'
probes=['array-index-enumeration.js','array-default-data-definitions.js','array-enumeration-descriptor-snapshot.js','class-field-proxy-descriptor.js','class-fields-and-typedarray-own-properties.js','proxy-private-class-elements.js']
command=[str(root/'scripts/native.sh'),*['conformance/native/'+name for name in probes]]
results=[]
for label,stress in [('ordinary',False),('stress',True)]:
 env=dict(os.environ)
 if stress: env['KILN_NATIVE_GC_STRESS']='1'
 started=time.time()
 with (dest/(label+'.log')).open('wb') as log: code=subprocess.call(command,env=env,stdout=log,stderr=subprocess.STDOUT)
 result={'label':label,'command':command,'exit_code':code,'wall_s':time.time()-started,'stress':stress}; results.append(result);(dest/(label+'-status.json')).write_text(json.dumps(result,indent=2)+'\n')
 if code:break
(dest/'probe-status.json').write_text(json.dumps(results,indent=2)+'\n')
raise SystemExit(any(result['exit_code'] for result in results))
