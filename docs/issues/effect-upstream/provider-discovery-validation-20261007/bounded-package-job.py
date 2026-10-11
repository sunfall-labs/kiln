#!/usr/bin/env python3
"""Run one prepared provider-package job with a bounded process group."""
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

spec_path = Path(sys.argv[1]).resolve()
spec = json.loads(spec_path.read_text())
install_dir = spec_path.parent
provenance = json.loads((install_dir / 'provenance.json').read_text())
log_path = Path(spec['log'])
running_path = Path(spec['running'])
status_path = Path(spec['status'])
for path in (log_path, running_path, status_path):
    path.parent.mkdir(parents=True, exist_ok=True)
if status_path.exists() or running_path.exists():
    raise SystemExit(f"refusing to overwrite prior job evidence: {status_path} or {running_path}")

def digest(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1 << 20), b''):
            h.update(block)
    return h.hexdigest()

def verify_source():
    mismatches = {}
    source_root = Path(provenance['source_root'])
    current = source_root / '.source-provenance.json'
    expected_current = spec['source_current_provenance_sha256']
    if digest(current) != expected_current:
        mismatches[str(current)] = {'expected': expected_current, 'actual': digest(current)}
    build = Path(provenance['source_identity']['build_source_provenance_path'])
    expected_build = spec['source_build_provenance_sha256']
    if digest(build) != expected_build:
        mismatches[str(build)] = {'expected': expected_build, 'actual': digest(build)}
    cli = install_dir / 'bin/kiln'
    expected_cli = provenance['cli_sha256']
    if digest(cli) != expected_cli:
        mismatches[str(cli)] = {'expected': expected_cli, 'actual': digest(cli)}
    for path, expected in provenance['provider_source_inputs_sha256'].items():
        target = source_root / path
        actual = digest(target)
        if actual != expected:
            mismatches[str(target)] = {'expected': expected, 'actual': actual}
    current_doc = json.loads(current.read_text())
    overlays = current_doc.get('overlays', [])
    for overlay in overlays:
        target = Path(overlay.get('immutable_v1_patch', overlay['path']))
        actual = digest(target)
        if actual != overlay['sha256']:
            mismatches[str(target)] = {'expected': overlay['sha256'], 'actual': actual}
    if mismatches:
        raise SystemExit('source/provider preflight mismatch: ' + json.dumps(mismatches, sort_keys=True))
    return {'current_provenance_sha256': digest(current), 'build_provenance_sha256': digest(build),
            'cli_sha256': digest(cli), 'verified_provider_inputs': len(provenance['provider_source_inputs_sha256']),
            'verified_overlays': len(overlays)}

source_preflight = verify_source()
started = time.time()
command = spec['command']
env = os.environ.copy()
env.update(spec.get('env', {}))
with log_path.open('xb') as log:
    child = subprocess.Popen(command, cwd=spec['cwd'], env=env, stdout=log,
                             stderr=subprocess.STDOUT, start_new_session=True)
    running = {
        'schema': 1, 'status': 'running', 'job_spec': str(spec_path),
        'job_spec_sha256': hashlib.sha256(spec_path.read_bytes()).hexdigest(),
        'supervisor_pid': os.getpid(), 'child_pid': child.pid, 'process_group_id': child.pid,
        'started_unix_s': started, 'timeout_s': 3600, 'command': command,
        'cwd': spec['cwd'], 'log': str(log_path), 'environment': spec.get('env', {}),
        'source_preflight': source_preflight
    }
    running_tmp = running_path.with_suffix(running_path.suffix + '.tmp')
    running_tmp.write_text(json.dumps(running, indent=2, sort_keys=True) + '\n')
    running_tmp.replace(running_path)
    timed_out = False
    termination_sent = False
    try:
        code = child.wait(timeout=3600)
    except subprocess.TimeoutExpired:
        timed_out = True
        termination_sent = True
        try:
            os.killpg(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            code = child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            code = child.wait()
    except BaseException:
        try:
            os.killpg(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait()
        raise
ended = time.time()
postflight_error = None
try:
    source_postflight = verify_source()
except BaseException as error:
    source_postflight = None
    postflight_error = repr(error)
result = dict(running, status='terminal', exit_code=code, timed_out=timed_out,
              termination_sent=termination_sent, ended_unix_s=ended,
              wall_s=ended-started, log_size_bytes=log_path.stat().st_size,
              source_postflight=source_postflight, source_postflight_error=postflight_error,
              integrity_valid=postflight_error is None)
tmp = status_path.with_suffix(status_path.suffix + '.tmp')
tmp.write_text(json.dumps(result, indent=2, sort_keys=True) + '\n')
tmp.replace(status_path)
final_code = 0 if code == 0 and not timed_out and postflight_error is None else (code if code != 0 else 1)
raise SystemExit(final_code)
