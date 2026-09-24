"""Exercise tap publication, retries and version ordering with a real Git remote."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/update-homebrew.sh'


def formula(version):
    # Like jetd.rb: no `version`, which Homebrew reads from the release URLs.
    return ('class Jetd < Formula\n'
            f'  url "https://github.com/apexgang/jet/releases/download/v{version}/jet-core-{version}.tar.gz"\n'
            'end\n')


def cask(version):
    return f'cask "jet-app" do\n  version "{version}"\nend\n'


class HomebrewPublication(unittest.TestCase):
    def test_publishes_formula_and_cask_once_and_never_downgrades(self):
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
    patterns = [args[i + 1] for i, arg in enumerate(args) if arg == '--pattern']
    assert patterns == ['jetd.rb', 'jet-app.rb'], patterns
    target = pathlib.Path(args[args.index('--dir') + 1])
    target.mkdir(parents=True, exist_ok=True)
    published = pathlib.Path(os.environ['TEST_PUBLISHED'])
    for name in patterns:
        shutil.copyfile(published / name, target / name)
elif args[:2] == ['release', 'view']:
    print(os.environ['TEST_LATEST'])
elif args[:2] == ['api', 'users/ape-bonker[bot]']:
    print('12345')
else:
    raise AssertionError(args)
''')
            gh.chmod(0o755)
            published = root / 'published'
            published.mkdir()

            def publish(version, latest=None):
                (published / 'jetd.rb').write_text(formula(version))
                (published / 'jet-app.rb').write_text(cask(version))
                env = dict(environment, PATH=f'{tools}{os.pathsep}{os.environ["PATH"]}',
                           GITHUB_REF_NAME=f'v{version}', RUNNER_TEMP=str(root / 'tmp'),
                           TEST_PUBLISHED=str(published), TEST_LATEST=f'v{latest or version}')
                subprocess.run(['bash', str(SCRIPT)], cwd=root, env=env, check=True,
                               capture_output=True, text=True)
                self.assertEqual(git('rev-parse', 'HEAD'), git('rev-parse', 'refs/remotes/origin/main'))
                return git('rev-parse', 'HEAD')

            def changed(commit):
                return git('show', '--format=', '--name-only', commit).split()

            first = publish('0.2.0')
            self.assertEqual(git('log', '-1', '--format=%s'), 'Updated Jet to v0.2.0')
            self.assertEqual(sorted(changed(first)), ['Casks/jet-app.rb', 'Formula/jetd.rb'])
            self.assertEqual(publish('0.2.0'), first)
            self.assertEqual(publish('0.1.0'), first)
            self.assertEqual(publish('0.3.0', latest='0.4.0'), first)
            second = publish('0.4.0')
            self.assertNotEqual(second, first)
            self.assertEqual(git('rev-parse', f'{second}~1'), first)
            self.assertEqual(sorted(changed(second)), ['Casks/jet-app.rb', 'Formula/jetd.rb'])
            self.assertEqual((tap / 'Formula/jetd.rb').read_text(), formula('0.4.0'))
            self.assertEqual((tap / 'Casks/jet-app.rb').read_text(), cask('0.4.0'))

            # Either file being newer skips both, so the tap never pairs the app
            # with another release's daemon. The formula's version comes from
            # its URLs, the cask's from its `version`.
            for ahead in ('Casks/jet-app.rb', 'Formula/jetd.rb'):
                with self.subTest(ahead=ahead):
                    (tap / 'Formula/jetd.rb').write_text(formula('0.9.0' if ahead.startswith('Formula') else '0.4.0'))
                    (tap / 'Casks/jet-app.rb').write_text(cask('0.9.0' if ahead.startswith('Casks') else '0.4.0'))
                    git('commit', '-am', f'Moved {ahead} ahead')
                    git('push', 'origin', 'main')
                    kept = git('rev-parse', 'HEAD')
                    pinned = {name: (tap / name).read_text() for name in ('Formula/jetd.rb', 'Casks/jet-app.rb')}
                    self.assertEqual(publish('0.5.0'), kept)
                    for name, text in pinned.items():
                        self.assertEqual((tap / name).read_text(), text)
                    self.assertEqual(git('status', '--porcelain'), '')
