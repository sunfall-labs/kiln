"""Reproduce Unicode measurements using the repository's capped Test262 runner."""
import argparse
import json
import pathlib
import sys

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / 'scripts'))
from test262_corpus import run_result, validate_checkout

parser = argparse.ArgumentParser()
parser.add_argument('--kiln', required=True)
parser.add_argument('--checkout', type=pathlib.Path, required=True)
parser.add_argument('--out', type=pathlib.Path, required=True)
parser.add_argument('--all', action='store_true')
args = parser.parse_args()
args.checkout = validate_checkout(args.checkout, minimum_files=1000)
files = [
    'CharacterClassEscapes/character-class-non-digit-class-escape-positive-cases.js',
    'property-escapes/generated/ASCII.js',
    'property-escapes/generated/Cased.js',
    'property-escapes/generated/General_Category_-_Letter.js',
    'property-escapes/generated/Script_-_Mongolian.js',
    'property-escapes/generated/Script_Extensions_-_Common.js',
]
if args.all:
    manifest = json.loads((REPO / 'conformance/test262-extended.json').read_text())
    files = sorted(manifest['chunks']['built-ins/RegExp']['files'])
    assert len(files) == 448
args.out.mkdir(parents=True, exist_ok=True)
selection = args.out / 'files.json'
selection.write_text(json.dumps(files) + '\n')
report, _ = run_result(
    str(pathlib.Path(args.kiln).resolve()),
    args.checkout / 'test/built-ins/RegExp',
    args.out / 'results.json', args.out / 'run.log', cases=selection,
)
expected = {f'{file}#{variant}' for file in files for variant in ('sloppy', 'strict')}
actual = [test['id'] for test in report['tests']]
assert len(actual) == len(expected) and set(actual) == expected
summary = {
    'elapsed_millis': report['_kiln_elapsed_millis'],
    'counts': report['_kiln_counts'],
    'summed_case_seconds': sum(test['elapsed_nanos'] for test in report['tests']) / 1e9,
}
(args.out / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print(json.dumps(summary))
