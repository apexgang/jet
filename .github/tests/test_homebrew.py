"""Exercise tap publication, retries and version ordering with a real Git remote."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/update-homebrew.sh'


class HomebrewPublication(unittest.TestCase):
    def test_publishes_once_and_never_downgrades(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tap = root / 'homebrew-tap'
            tap.mkdir()
            environment = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull,
                               GIT_CONFIG_SYSTEM=os.devnull)

            def git(*args):
                return subprocess.check_output(['git', *args], cwd=tap, env=environment, text=True,
                                               stderr=subprocess.PIPE).strip()

            git('init', '--bare', '--initial-branch=main', str(root / 'remote'))
            git('init', '--initial-branch=main')
            git('config', 'user.name', 'Test')
            git('config', 'user.email', 'test@example.invalid')
            git('commit', '--allow-empty', '-m', 'Initialized test tap')
            git('remote', 'add', 'origin', str(root / 'remote'))
            git('push', 'origin', 'main')
            tools = root / 'bin'
            tools.mkdir()
            gh = tools / 'gh'
            gh.write_text('''#!/usr/bin/env python3
import os, pathlib, shutil, sys
args = sys.argv[1:]
if args[:2] == ['release', 'download']:
    assert args[args.index('--pattern') + 1] == 'jet.rb'
    target = pathlib.Path(args[args.index('--dir') + 1])
    target.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(os.environ['TEST_FORMULA'], target / 'jet.rb')
elif args[:2] == ['release', 'view']:
    print(os.environ['TEST_LATEST'])
elif args[:2] == ['api', 'users/ape-bonker[bot]']:
    print('12345')
else:
    raise AssertionError(args)
''')
            gh.chmod(0o755)
            source = root / 'published.rb'

            def publish(version, latest=None):
                source.write_text(f'class Jet\n  version "{version}"\nend\n')
                env = dict(environment, PATH=f'{tools}{os.pathsep}{os.environ["PATH"]}',
                           GITHUB_REF_NAME=f'v{version}', RUNNER_TEMP=str(root / 'tmp'),
                           TEST_FORMULA=str(source), TEST_LATEST=f'v{latest or version}')
                subprocess.run(['bash', str(SCRIPT)], cwd=root, env=env, check=True,
                               capture_output=True, text=True)
                self.assertEqual(git('rev-parse', 'HEAD'), git('rev-parse', 'refs/remotes/origin/main'))
                return git('rev-parse', 'HEAD')

            first = publish('0.2.0')
            self.assertEqual(git('log', '-1', '--format=%s'), 'Updated Jet to v0.2.0')
            self.assertEqual(publish('0.2.0'), first)
            self.assertEqual(publish('0.1.0'), first)
            self.assertEqual(publish('0.3.0', latest='0.4.0'), first)
            self.assertNotEqual(publish('0.4.0'), first)
            self.assertEqual((tap / 'Formula/jet.rb').read_text(), source.read_text())
