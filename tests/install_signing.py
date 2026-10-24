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
  if '--requirements' in args:
   requirement='fixture-existing'
   if os.environ.get('MISMATCH_IDENTITY') and '.reap-install.' in args[-1]: requirement='fixture-different'
   print('designated => '+requirement)
  elif os.environ.get('EXISTING_SIGNED'): print('Signature size=4672\\nTeamIdentifier=fixture')
  else: print('Signature=adhoc')
 elif '--sign' in args:
  if os.environ.get('FAIL_SIGN'): sys.exit(12)
 elif '--verify' in args:
  if os.environ.get('FAIL_VERIFY'): sys.exit(13)
'''

LIFECYCLE = '''import json,os,sys
from pathlib import Path
action=sys.argv[1]
arguments=sys.argv[2:]
binary=Path(arguments[arguments.index('--binary')+1])
with Path(os.environ['TRACE']).open('a') as stream: stream.write('lifecycle '+action+'\\n')
if action=='preflight':
 if os.environ.get('FAIL_SERVICE_PREFLIGHT'): sys.exit(14)
 candidate=Path(arguments[arguments.index('--candidate')+1])
 assert candidate.is_file()
 Path(arguments[arguments.index('--plan')+1]).write_text(json.dumps({'fixture':True}))
elif action=='apply':
 assert binary.read_text().startswith('#!/bin/sh')
 if os.environ.get('FAIL_SERVICE_APPLY'): sys.exit(15)
else: sys.exit(2)
'''


def check(case):
    evidence = ROOT / 'target/control-t13'
    evidence.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='reap-install-test-', dir=evidence) as temporary:
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
        (project / 'tools').mkdir()
        (project / 'tools/install_services_macos.py').write_text(LIFECYCLE)
        for name in ('cargo', 'codesign', 'uname'):
            path = commands / name
            path.write_text(HELPER)
            path.chmod(0o755)
        (home / '.cargo/bin').mkdir(parents=True)
        (home / '.claude/skills').mkdir(parents=True)
        (home / '.codex/skills').mkdir(parents=True)
        installed = home / '.cargo/bin/reap'
        old = b'previous installed executable'
        prebuilt = case.startswith('prebuilt_')
        failed = case in ('build_failure', 'sign_failure', 'verify_failure', 'unsigned_downgrade',
                          'prebuilt_verify_failure', 'prebuilt_identity_mismatch', 'prebuilt_missing',
                          'service_preflight_failure', 'service_apply_failure')
        if failed:
            installed.write_bytes(old)
        preserved = {}
        for agent in ('claude', 'codex'):
            skill = home / ('.' + agent) / 'skills/reap/SKILL.md'
            if failed or case in ('existing_skill', 'existing_skill_symlink', 'dangling_skill_symlink'):
                skill.parent.mkdir()
                content = agent + ' operator-owned skill'
                if case in ('existing_skill_symlink', 'dangling_skill_symlink'):
                    target = home / (agent + '-operator-skill.md')
                    if case == 'existing_skill_symlink':
                        target.write_text(content)
                    skill.symlink_to(target)
                else:
                    skill.write_text(content)
                preserved[skill] = content
        env = dict(os.environ, HOME=str(home), TRACE=str(base / 'trace'),
                   PATH=str(commands) + ':' + str(home / '.cargo/bin') + ':' + os.environ['PATH'])
        for key in ('CARGO_HOME', FIXTURE['environment'], 'FAIL_BUILD', 'FAIL_SIGN',
                    'FAIL_VERIFY', 'EXISTING_SIGNED', 'TEST_PLATFORM', 'MISMATCH_IDENTITY',
                    'FAIL_SERVICE_PREFLIGHT', 'FAIL_SERVICE_APPLY'):
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
        if case == 'service_preflight_failure':
            env['FAIL_SERVICE_PREFLIGHT'] = '1'
        if case == 'service_apply_failure':
            env['FAIL_SERVICE_APPLY'] = '1'
        command = ['/bin/bash', str(project / 'install.sh')]
        if prebuilt:
            artifact = base / 'signed input with spaces'
            if case != 'prebuilt_missing':
                artifact.write_text('#!/bin/sh\nprintf "reap fixture\\n"\n')
                artifact.chmod(0o755)
            command += ['--binary', str(artifact)]
            env['FAIL_BUILD'] = '1'
            if case != 'prebuilt_linux':
                env['EXISTING_SIGNED'] = '1'
            else:
                env['TEST_PLATFORM'] = 'Linux'
            if case == 'prebuilt_identity_mismatch':
                env['MISMATCH_IDENTITY'] = '1'
            if case == 'prebuilt_verify_failure':
                env['FAIL_VERIFY'] = '1'
            if not installed.exists():
                installed.write_bytes(old)
        result = subprocess.run(command, env=env,
                                capture_output=True, text=True)
        trace = (base / 'trace').read_text()
        assert (result.returncode != 0) == failed, (case, result.stdout, result.stderr)
        if failed and case != 'service_apply_failure':
            assert installed.read_bytes() == old, case
        else:
            assert subprocess.check_output([str(installed)], text=True).strip() == 'reap fixture'
        for agent in ('claude', 'codex'):
            skill = home / ('.' + agent) / 'skills/reap/SKILL.md'
            if case == 'dangling_skill_symlink':
                assert skill.is_symlink() and not skill.exists(), case
                assert not skill.readlink().exists(), case
            elif skill in preserved:
                assert skill.read_text() == preserved[skill], case
                assert skill.is_symlink() == (case == 'existing_skill_symlink'), case
            elif not failed:
                assert skill.read_text() == 'fixture skill', case
        if case == 'repeat_install':
            (project / 'skill/SKILL.md').write_text('different bundled skill')
            repeated = subprocess.run(['/bin/bash', str(project / 'install.sh')], env=env,
                                      capture_output=True, text=True)
            assert repeated.returncode == 0, (case, repeated.stdout, repeated.stderr)
            for agent in ('claude', 'codex'):
                assert (home / ('.' + agent) / 'skills/reap/SKILL.md').read_text() == 'fixture skill'
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
        if case in ('linux', 'prebuilt_linux'):
            assert 'lifecycle ' not in trace, trace
        elif not failed or case == 'service_apply_failure':
            assert trace.index("'--verify'") < trace.index('lifecycle preflight'), trace
            assert trace.index('lifecycle preflight') < trace.index('lifecycle apply'), trace
        if case == 'service_preflight_failure':
            assert 'lifecycle preflight' in trace and 'lifecycle apply' not in trace, trace
        if prebuilt:
            assert 'cargo ' not in trace and "'--sign'" not in trace, trace
            if case == 'prebuilt_linux':
                assert 'codesign ' not in trace, trace
        assert not list((home / '.cargo').glob('.reap-install.*')), case
        assert not list((home / '.cargo/bin').glob('.reap-install.*')), case
        print(case + ': passed')


for case in FIXTURE['cases']:
    check(case)
