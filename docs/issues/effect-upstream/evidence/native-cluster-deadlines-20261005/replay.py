import hashlib, importlib.util, json, sys
from pathlib import Path
root=Path.cwd()
spec=importlib.util.spec_from_file_location('effect_upstream',root/'scripts/effect-upstream.py')
runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(runner)
destination=root/'.scratch/frozen-clock-native/cluster-hook-contract-native'
workspace=root/'.scratch/frozen-clock-native/workspace'
inputs=json.loads((root/'.scratch/frozen-clock-native/cluster-hook-contract-interpreter/report.json').read_text())
command=[sys.executable,str(root/'scripts/effect-upstream.py'),'--limited-child','8192',str(destination/'native.bin')]
envelope=runner.execute(command,workspace,destination/'native.log',1800,1)
envelope.update(runner.records(destination/'native.log'))
envelope['matches_expected_exit'] = envelope['exit_code']==(1 if envelope.get('summary',{}).get('failed',0) else 0)
comparison=runner.compare_node_vitest(envelope,'cluster/ClusterWorkflowEngine',inputs['source_sha256']['cluster/ClusterWorkflowEngine'])
report={'schema':1,'scope':'one original ClusterWorkflowEngine file; quiet team baseline, not final combined source',
'pin':inputs['pin'],'input_mode':'direct-typescript','cli_source':'faff5879',
'cli_sha256':inputs['kiln_binary_sha256'],'native_profile':'native-release',
'binary_sha256':hashlib.sha256((destination/'native.bin').read_bytes()).hexdigest(),
'harness_sha256':inputs['harness_sha256'],'source_sha256':inputs['source_sha256'],
'node_vitest_oracle':inputs['node_vitest_oracle'],'native_provider':inputs['native_provider'],
'build_log':str(destination/'build.log'),'outer_timeout_s':1800,'runtime_jobs':1,'memory_mb':8192,
'engine':envelope,'node_vitest':comparison}
(destination/'report.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({'execution':envelope.get('summary'),'complete':envelope['complete'],'node_vitest':comparison},indent=2))
sys.exit(0 if comparison.get('matches_outcomes') is True and envelope['complete'] else 1)
