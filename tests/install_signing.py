import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
FIXTURE = json.loads((ROOT / 'docs/specs/macos-install.fixtures.json').read_text())
HELPER = '''#!/usr/bin/env python3
import os,sys
from pathlib import Path
name=Path(sys.argv[0]).name
args=sys.argv[1:]
with Path(os.environ['TRACE']).open('a') as stream: stream.write(name+' '+repr(args)+'\\n')
if name=='uname': print(os.environ.get('TEST_PLATFORM','Darwin'))
elif name=='cargo':
 if os.environ.get('FAIL_BUILD'): sys.exit(11)
 root=Path(args[args.index('--root')+1]); (root/'bin').mkdir()
 output=root/'bin/reap'
 output.write_text('#!/bin/sh\\nprintf "reap fixture\\\\n"\\n')
 output.chmod(0o755)
elif name=='codesign':
 if '--display' in args:
  if os.environ.get('EXISTING_SIGNED'): print('Authority=Developer ID Application: fixture')
 elif '--sign' in args:
  if os.environ.get('FAIL_SIGN'): sys.exit(12)
 elif '--verify' in args:
  if os.environ.get('FAIL_VERIFY'): sys.exit(13)
'''


def check(case):
    with tempfile.TemporaryDirectory(prefix='reap-install-test-') as temporary:
        base = Path(temporary)
        project = base / 'source with spaces'
        home = base / 'home'
        commands = base / 'commands'
        project.mkdir()
        commands.mkdir()
        (project / 'Cargo.toml').touch()
        (project / 'skill').mkdir()
        (project / 'skill/SKILL.md').write_text('fixture skill')
        shutil.copy2(ROOT / 'install.sh', project / 'install.sh')
        for name in ('cargo', 'codesign', 'uname'):
            path = commands / name
            path.write_text(HELPER)
            path.chmod(0o755)
        (home / '.cargo/bin').mkdir(parents=True)
        (home / '.codex/skills').mkdir(parents=True)
        installed = home / '.cargo/bin/reap'
        old = b'previous installed executable'
        if case in ('build_failure', 'sign_failure', 'verify_failure', 'unsigned_downgrade'):
            installed.write_bytes(old)
        env = dict(os.environ, HOME=str(home), TRACE=str(base / 'trace'),
                   PATH=str(commands) + ':' + str(home / '.cargo/bin') + ':' + os.environ['PATH'])
        for key in ('CARGO_HOME', FIXTURE['environment'], 'FAIL_BUILD', 'FAIL_SIGN',
                    'FAIL_VERIFY', 'EXISTING_SIGNED', 'TEST_PLATFORM'):
            env.pop(key, None)
        if case not in ('fresh', 'build_failure', 'unsigned_downgrade'):
            (project / '.codesign.env').write_text('REAP_CODESIGN_IDENTITY="fixture-file"\n')
        if case == 'environment_override':
            env[FIXTURE['environment']] = 'fixture-environment'
        for label, variable in (('build_failure', 'FAIL_BUILD'), ('sign_failure', 'FAIL_SIGN'),
                                ('verify_failure', 'FAIL_VERIFY'), ('unsigned_downgrade', 'EXISTING_SIGNED')):
            if case == label:
                env[variable] = '1'
        if case == 'linux':
            env['TEST_PLATFORM'] = 'Linux'
        result = subprocess.run(['/bin/bash', str(project / 'install.sh')], env=env,
                                capture_output=True, text=True)
        trace = (base / 'trace').read_text()
        failed = case in ('build_failure', 'sign_failure', 'verify_failure', 'unsigned_downgrade')
        assert (result.returncode != 0) == failed, (case, result.stdout, result.stderr)
        if failed:
            assert installed.read_bytes() == old, case
        else:
            assert subprocess.check_output([str(installed)], text=True).strip() == 'reap fixture'
            assert (home / '.codex/skills/reap/SKILL.md').read_text() == 'fixture skill'
        if case in ('configured', 'environment_override'):
            identity = 'fixture-environment' if case == 'environment_override' else 'fixture-file'
            assert repr(FIXTURE['identifier']) in trace and repr(identity) in trace, trace
            assert trace.index("'--sign'") < trace.index("'--verify'"), trace
        if case == 'fresh':
            assert "'--sign', '-'" in trace, trace
        if case in ('linux', 'unsigned_downgrade'):
            assert "'--sign'" not in trace, trace
        if case == 'unsigned_downgrade':
            assert 'cargo ' not in trace, trace
        if case == 'linux':
            assert 'codesign ' not in trace, trace
        assert not list((home / '.cargo').glob('.reap-install.*')), case
        assert not list((home / '.cargo/bin').glob('.reap-install.*')), case
        print(case + ': passed')


for case in FIXTURE['cases']:
    check(case)
