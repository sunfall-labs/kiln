import json, hashlib
from pathlib import Path
root=Path(__file__).resolve().parent
names=["matched-published-control","affected-cohort"]
reports={}
for name in names:
    status=json.loads((root/name/"status.json").read_text())
    assert status["status"]=="terminal"
    r=json.loads((root/name/"report.json").read_text())
    a=r["aggregate"]["engines"]["interpreter"]
    assert r["aggregate"]["reported_files"]==7 and r["aggregate"]["pending_files"]==0
    assert a["recorded_cases"]==a["registered_cases"]==a["node_vitest_mapped_cases"]==562
    assert a["node_vitest_mapping_errors"]==a["node_vitest_mode_mismatches"]==a["node_vitest_unmapped_cases"]==0
    reports[name]=r
control,candidate=[reports[x] for x in names]
for key in ["pin","selected_files","engine_mode_policy","clock_policy","harness_sha256","source_sha256","runner_sha256"]:
    assert control[key]==candidate[key],key
byfile=[{f["file"]:f["engines"]["interpreter"] for f in r["files"]} for r in [control,candidate]]
assert byfile[0].keys()==byfile[1].keys()
rows=[]
for file in byfile[0]:
    old,new=[d[file] for d in byfile]
    cases=[{c["id"]:c["outcome"] for c in e["cases"]} for e in [old,new]]
    assert cases[0].keys()==cases[1].keys()
    rows.append({"file":file,"control_summary":old["summary"],"candidate_summary":new["summary"],"control_wall_s":old["wall_s"],"candidate_wall_s":new["wall_s"],"changed_outcomes":[{"id":id,"control":cases[0][id],"candidate":cases[1][id]} for id in cases[0] if cases[0][id]!=cases[1][id]]})
result={"scope":"Seven original files, 562 cases; serial single control/candidate trials, original deadlines, not whole-suite acceptance or isolated repeatable performance attribution","inputs":{name:{"report_sha256":hashlib.sha256((root/name/"report.json").read_bytes()).hexdigest(),"cli_sha256":reports[name]["kiln_binary_sha256"],"aggregate":reports[name]["aggregate"]} for name in names},"files":rows}
(root/"matched-comparison.json").write_text(json.dumps(result,indent=2)+"\n")
print(json.dumps(result,indent=2))
