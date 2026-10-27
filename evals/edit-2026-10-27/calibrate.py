"""Run an isolated, non-task Codex calibration with the frozen arm profile."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))

import comparison_campaign as cc
from comparison_metrics import summarize_trace
import edit_evaluation as evaluation
import edit_sandbox
import edit_snapshot


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--arm', choices=('native', 'code-kb'), required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--artifact-paths', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists():
        raise ValueError('calibration output already exists')
    output.mkdir(parents=True)
    artifacts = json.loads(args.artifact_paths.read_text())
    manifest_path = ROOT / 'evals/edit-2026-10-27/manifest.json'
    validated = evaluation.validate_manifest(manifest_path, artifact_paths=artifacts)
    manifest = json.loads(manifest_path.read_text())
    codex_home = Path(os.environ.get('CODEX_HOME', str(Path.home() / '.codex')))
    auth_file = codex_home / 'auth.json'
    private = output / 'private-profile'
    shutil.copytree(ROOT / 'evals/edit-2026-10-27/profiles' / args.arm, private)
    (private / 'calibration-canary.txt').write_text('private calibration canary\n')
    seed = output / 'seed-project'
    (seed / 'src').mkdir(parents=True)
    (seed / 'pyproject.toml').write_text('[project]\nname="codex-calibration"\nversion="0.1.0"\n')
    (seed / 'src/probe.py').write_text('def result():\n    return 1\n')
    (seed / 'probe_privacy.py').write_text(
        'import errno, json, os, socket\n'
        'from pathlib import Path\n'
        'home = Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex")))\n'
        'private_denied = False\n'
        'try:\n'
        '    (home / "calibration-canary.txt").read_text()\n'
        'except PermissionError:\n'
        '    private_denied = True\n'
        'network_denied = False\n'
        'try:\n'
        '    connection = socket.create_connection(("1.1.1.1", 443), timeout=1)\n'
        '    connection.close()\n'
        'except OSError as error:\n'
        '    network_denied = error.errno in (errno.EPERM, errno.EACCES)\n'
        'result = {"private_denied": private_denied, "network_denied": network_denied}\n'
        'Path("/state/permissions.json").write_text(json.dumps(result))\n'
        'assert private_denied and network_denied, result\n'
        'print("PERMISSIONS_OK")\n'
    )
    evaluation.initialize_trial_git(seed)
    project = output / 'project'
    shutil.copytree(seed, project)
    snapshot = edit_snapshot.snapshot_tree(seed, excluded=('.code-kb', '__pycache__', 'src/__pycache__'))
    state = output / 'state'
    tools = {'codex': Path(artifacts['codex']), 'codex-code-mode-host': Path(artifacts['codex_code_mode_host'])}
    cold = None
    if args.arm == 'code-kb':
        tools['intelligence'] = Path(artifacts['binary']).parent
        cold = edit_sandbox.run(project, state, output / 'preindex', 120, 'grade',
            ['/usr/bin/env', 'JULIE_EXTRACT_BIN=/tools/intelligence/julie-extract', '/tools/intelligence/code-kb', '--root', '/workspace/project', 'scan'],
            tools={'intelligence': tools['intelligence']})
        if cold['exit_status'] or cold['timed_out']:
            raise ValueError('calibration preindex failed')
    prompt = (
        'This is a non-task execution calibration in /workspace/project. '
        'Change src/probe.py so result() returns 2. Create verification.txt containing verified and a newline. '
        'Run the existing protected probe_privacy.py with python3; it verifies private-profile reads and command networking are denied and must print PERMISSIONS_OK. '
        'Do not modify that file or Git metadata. Verify the edited result using python3. Finish with a short report. '
        + ('Use the code-kb MCP tools first for source structure and pass project_root=/workspace/project.' if args.arm == 'code-kb' else 'Use native terminal tools for source inspection.')
    )
    assert cc.redact_bytes(prompt.encode()) == prompt.encode()
    (output / 'prompt.txt').write_text(prompt)
    argv = ['codex', 'exec', '--json', '--ephemeral', '--ignore-rules', '--color', 'never', '-m', manifest['controls']['model'], '-o', '/state/final.txt', prompt]
    print(json.dumps({'stage': 'dispatch', 'arm': args.arm, 'output': str(output)}), flush=True)
    execution = edit_sandbox.run(project, state, output / 'agent', 180, 'agent', argv,
        tools=tools, protect=('.git', 'probe_privacy.py'), private_profile=private, auth_file=auth_file)
    events, errors = cc._read_trace(Path(execution['stdout_log']))
    metrics = summarize_trace(events)
    bundle = edit_snapshot.capture_bundle(snapshot, project, output / 'bundle', protected=('.git', 'probe_privacy.py'), excluded=('.code-kb', '__pycache__', 'src/__pycache__'))
    replay = output / 'replayed-project'
    edit_snapshot.apply_bundle(seed, output / 'bundle', replay, protected=('.git', 'probe_privacy.py'), excluded=('.code-kb', '__pycache__', 'src/__pycache__'))
    grade = edit_sandbox.run(replay, output / 'grade-state', output / 'grade', 30, 'grade',
        ['/usr/bin/python3', '-c', 'import runpy; from pathlib import Path; assert runpy.run_path("src/probe.py")["result"]() == 2; assert Path("verification.txt").read_text() == "verified\\n"; print("CALIBRATION_OK")'])
    permission_path = state / 'permissions.json'
    permissions = json.loads(permission_path.read_text()) if permission_path.exists() else {}
    passed = bool(execution['exit_status'] == 0 and not execution['timed_out'] and metrics['completed'] and not errors and not bundle['violations'] and grade['exit_status'] == 0 and 'CALIBRATION_OK' in Path(grade['stdout_log']).read_text() and permissions == {'private_denied': True, 'network_denied': True})
    if args.arm == 'code-kb':
        passed = passed and metrics['code_kb_calls'] > 0
    else:
        passed = passed and not metrics['mcp_calls']
    record = {'schema': 1, 'arm': args.arm, 'status': 'passed' if passed else 'failed', 'scored': False, 'provider_calls': 1, 'validated_manifest': validated, 'execution': execution, 'metrics': metrics, 'parse_errors': errors, 'permissions': permissions, 'grade': grade, 'bundle_violations': bundle['violations'], 'cold_index': cold, 'manifest_sha256': hashlib.sha256(manifest_path.read_bytes()).hexdigest()}
    (output / 'calibration.json').write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps({'status': record['status'], 'record': str(output / 'calibration.json')}), flush=True)
    return 0 if passed else 1


if __name__ == '__main__':
    raise SystemExit(main())
