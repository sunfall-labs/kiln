import sys, pathlib, json, os
sys.path.insert(0, "scripts")
from test262_corpus import run_result
os.environ["KILN_JOBS"]="4"
os.environ["KILN_TEST262_MEMORY_MB"]="4096"
p=pathlib.Path("docs/issues/extended-mixed")
r,c=run_result(sys.argv[1], pathlib.Path("~/.cache/kiln/test262-771005236e/test"),p/(sys.argv[2]+".json"),p/(sys.argv[2]+".log"),cases=p/(sys.argv[3] if len(sys.argv)>3 else "cases.json"))
print(r["totals"],r["_kiln_elapsed_millis"])
