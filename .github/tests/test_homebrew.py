"""Exercise tap publication, retries and version ordering with a real Git remote.

The tap is shared with the Swift app's release: `publish-swift-cask.sh` owns
`Casks/jet.rb` and writes a macOS-only `Formula/jet.rb` from the same
template. These tests run that script unchanged next to `update-homebrew.sh`.
"""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1] / 'scripts'
SCRIPT = SCRIPTS / 'update-homebrew.sh'
sys.path.insert(0, str(SCRIPTS))
import package_swift_app
import release_assets

# How publish-swift-cask.sh reads a formula's core version.
SWIFT_VERSION_LINE = re.compile(r'^  version "([0-9]+\.[0-9]+\.[0-9]+)"$', re.MULTILINE)

FAKE_GH = '''#!/usr/bin/env python3
import os, pathlib, shutil, sys
args = sys.argv[1:]
if args[:2] == ['release', 'download']:
    patterns = [args[i + 1] for i, arg in enumerate(args) if arg == '--pattern']
    assert patterns == ['jet.rb', 'jet-app.rb'], patterns
    assert args[args.index('--repo') + 1] == 'apexgang/jet', args
    target = pathlib.Path(args[args.index('--dir') + 1])
    target.mkdir(parents=True, exist_ok=True)
    published = pathlib.Path(os.environ['TEST_PUBLISHED']) / args[2]
    with open(os.environ['TEST_DOWNLOADS'], 'a') as log:
        log.write(args[2] + '\\n')
    for name in patterns:
        shutil.copyfile(published / name, target / name)
elif args[:2] == ['release', 'list']:
    assert args[args.index('--json') + 1] == 'tagName,isDraft,isPrerelease', args
    print(os.environ['TEST_RELEASES'])
elif args[:2] == ['api', 'users/ape-bonker[bot]']:
    print('12345')
else:
    raise AssertionError(args)
'''


def rendered(version):
    """The tagged release's formula and cask, rendered by release_assets.py."""
    url = f'https://github.com/apexgang/jet/releases/download/v{version}'
    return release_assets.render(release_assets.replacements(version, url, {}, {}))


def releases(*tags, drafts=(), prereleases=()):
    """`gh release list --json tagName,isDraft,isPrerelease` output."""
    return json.dumps([{'tagName': tag, 'isDraft': tag in drafts, 'isPrerelease': tag in prereleases}
                       for tag in (*tags, *drafts, *prereleases)])


class HomebrewPublication(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.remote = self.root / 'remote'
        self.environment = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull)
        tools = self.root / 'bin'
        tools.mkdir()
        (tools / 'gh').write_text(FAKE_GH)
        (tools / 'gh').chmod(0o755)
        self.environment['PATH'] = f'{tools}{os.pathsep}{os.environ["PATH"]}'
        self.published = self.root / 'published'
        self.published.mkdir()
        seed = self.root / 'seed'
        seed.mkdir()
        self.git(seed, 'init', '--bare', '--initial-branch=main', str(self.remote))
        self.git(seed, 'init', '--initial-branch=main')
        self.git(seed, 'config', 'user.name', 'Test')
        self.git(seed, 'config', 'user.email', 'test@example.invalid')
        self.git(seed, 'commit', '--allow-empty', '-m', 'Initialized test tap')
        self.git(seed, 'remote', 'add', 'origin', str(self.remote))
        self.git(seed, 'push', 'origin', 'main')

    def git(self, tap, *args):
        return subprocess.check_output(['git', *args], cwd=tap, env=self.environment, text=True,
                                       stderr=subprocess.PIPE).strip()

    def checkout(self, workspace):
        """A fresh `homebrew-tap` clone under `workspace`, as each job's checkout makes it."""
        tap = workspace / 'homebrew-tap'
        shutil.rmtree(tap, ignore_errors=True)
        workspace.mkdir(exist_ok=True)
        subprocess.run(['git', 'clone', '--branch', 'main', str(self.remote), str(tap)],
                       env=self.environment, check=True, capture_output=True)
        self.git(tap, 'config', 'user.name', 'Test')
        self.git(tap, 'config', 'user.email', 'test@example.invalid')
        return tap

    def remote_file(self, path):
        return self.git(self.remote, 'show', f'main:{path}') + '\n'

    def remote_head(self):
        return self.git(self.remote, 'rev-parse', 'main')

    def changed(self, commit):
        return sorted(self.git(self.remote, 'show', '--format=', '--name-only', commit).split())

    def swift(self, app_version, core_version, workspace=None):
        """Runs main's Swift tap publication (publish-swift-cask.sh) from a fresh checkout."""
        workspace = workspace or self.root / 'swift'
        self.checkout(workspace)
        dist = workspace / 'swift-dist'
        dist.mkdir(exist_ok=True)
        (dist / 'jet.rb').write_text(package_swift_app.cask(app_version, 'a' * 64))
        (dist / 'jet-core.rb').write_text(package_swift_app.mac_formula(core_version, 'b' * 64, app_version))
        subprocess.run(['bash', str(SCRIPTS / 'publish-swift-cask.sh'), app_version], cwd=workspace,
                       env=self.environment, check=True, capture_output=True, text=True)

    def update(self, version, listed, fresh=True, check=True):
        """Runs update-homebrew.sh for tag `v<version>` with `listed` as the repository's releases.

        Every listed core release publishes its own formula and cask; the
        result's `downloaded` names the releases the script downloaded.
        """
        workspace = self.root / 'core'
        if fresh:
            self.checkout(workspace)
        for release in json.loads(listed):
            match = re.fullmatch(r'v([0-9]+\.[0-9]+\.[0-9]+)', release['tagName'])
            if match:
                (self.published / release['tagName']).mkdir(exist_ok=True)
                for name, text in rendered(match[1]).items():
                    (self.published / release['tagName'] / name).write_text(text)
        downloads = self.root / 'downloads'
        downloads.write_text('')
        env = dict(self.environment, GITHUB_REF_NAME=f'v{version}', RUNNER_TEMP=str(self.root / 'tmp'),
                   TEST_PUBLISHED=str(self.published), TEST_RELEASES=listed, TEST_DOWNLOADS=str(downloads))
        result = subprocess.run(['bash', str(SCRIPT)], cwd=workspace, env=env, capture_output=True, text=True)
        result.downloaded = downloads.read_text().split()
        if check:
            self.assertEqual(result.returncode, 0, result.stderr)
        return result

    def test_replaces_the_swift_formula_and_never_touches_the_swift_cask(self):
        # The tap as the Swift release left it: a macOS-only formula at core
        # 0.2.0, published from a swift-v tag, and the macOS app's cask.
        self.swift('1.0.4', '0.2.0')
        swift_formula = self.remote_file('Formula/jet.rb')
        swift_cask = self.remote_file('Casks/jet.rb')
        self.assertIn('depends_on :macos', swift_formula)
        self.assertIn('/swift-v1.0.4/', swift_formula)

        # Swift app releases are listed too; only the core tag counts.
        self.update('0.2.0', releases('swift-v1.0.4', 'v0.2.0'))
        first = self.remote_head()
        self.assertEqual(self.git(self.remote, 'log', '-1', '--format=%s', first), 'Updated Jet to v0.2.0')
        self.assertEqual(self.changed(first), ['Casks/jet-app.rb', 'Formula/jet.rb'])
        formula = self.remote_file('Formula/jet.rb')
        self.assertEqual(formula, rendered('0.2.0')['jet.rb'])
        self.assertNotIn('depends_on :macos', formula)
        for label in release_assets.LABELS:
            self.assertIn(f'/v0.2.0/jet-core-0.2.0-{label}.tar.gz"', formula)
        self.assertEqual(SWIFT_VERSION_LINE.findall(formula), ['0.2.0'])
        self.assertEqual(self.remote_file('Casks/jet-app.rb'), rendered('0.2.0')['jet-app.rb'])
        self.assertEqual(self.remote_file('Casks/jet.rb'), swift_cask)

        # Reruns change nothing.
        result = self.update('0.2.0', releases('swift-v1.0.4', 'v0.2.0'))
        self.assertIn('already has this release', result.stdout)
        self.assertEqual(self.remote_head(), first)

        # The Swift release replaces the formula only for a newer core, so a
        # new app build at the same core updates its cask alone.
        self.swift('1.0.5', '0.2.0')
        self.assertEqual(self.changed(self.remote_head()), ['Casks/jet.rb'])
        self.assertEqual(self.remote_file('Formula/jet.rb'), formula)
        self.assertIn('version "1.0.5"', self.remote_file('Casks/jet.rb'))

    def test_never_downgrades_and_skips_both_when_either_is_newer(self):
        self.update('0.2.0', releases('v0.2.0'))
        kept = self.remote_head()
        # An older tag brings the tap to the highest stable core release, which
        # it already holds; when that release is not listed, the tap's
        # versions keep it.
        result = self.update('0.1.0', releases('v0.1.0', 'v0.2.0', 'swift-v1.0.9'))
        self.assertEqual(result.downloaded, ['v0.2.0'])
        self.assertIn('already has this release', result.stdout)
        result = self.update('0.1.0', releases('v0.1.0', 'swift-v1.0.9'))
        self.assertEqual(result.downloaded, ['v0.1.0'])
        self.assertIn("the tap's Formula/jet.rb is newer", result.stdout)
        self.assertEqual(self.remote_head(), kept)

        # Drafts, prereleases and Swift app tags never count as the latest
        # core release.
        result = self.update('0.3.0', releases('v0.2.0', 'v0.3.0', 'v1.0.0-beta', 'swift-v1.0.9',
                                               drafts=('v0.9.0',), prereleases=('v0.4.0-rc.1',)))
        self.assertEqual(result.downloaded, ['v0.3.0'])
        self.assertEqual(self.remote_file('Formula/jet.rb'), rendered('0.3.0')['jet.rb'])
        kept = self.remote_head()
        result = self.update('0.3.0', releases('swift-v1.0.9', drafts=('v0.3.0',)), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('No stable core release', result.stderr)
        self.assertEqual(result.downloaded, [])
        self.assertEqual(self.remote_head(), kept)

        # A newer cask alone also skips both.
        seed = self.checkout(self.root / 'maintainer')
        (seed / 'Casks/jet-app.rb').write_text(rendered('0.9.0')['jet-app.rb'])
        self.git(seed, 'commit', '-am', 'Moved the cask ahead')
        self.git(seed, 'push', 'origin', 'main')
        kept = self.remote_head()
        result = self.update('0.5.0', releases('v0.5.0'))
        self.assertIn("the tap's Casks/jet-app.rb is newer", result.stdout)
        self.assertEqual(self.remote_head(), kept)
        self.assertEqual(self.remote_file('Formula/jet.rb'), rendered('0.3.0')['jet.rb'])

        # Versions compare numerically: 0.10.0 is newer than the cask's 0.9.0.
        result = self.update('0.9.0', releases('v0.9.0', 'v0.10.0'))
        self.assertEqual(result.downloaded, ['v0.10.0'])
        self.assertEqual(self.git(self.remote, 'log', '-1', '--format=%s', 'main'), 'Updated Jet to v0.10.0')
        self.assertEqual(self.remote_file('Formula/jet.rb'), rendered('0.10.0')['jet.rb'])
        self.assertEqual(self.remote_file('Casks/jet-app.rb'), rendered('0.10.0')['jet-app.rb'])

    def test_any_run_brings_the_tap_to_the_highest_release(self):
        # The tap jobs share one concurrency group, and GitHub keeps one
        # pending job per group: a backport tagged after v0.3.1 cancels
        # v0.3.1's pending tap job. The backport's run publishes v0.3.1's
        # formula and cask in one commit instead of skipping.
        self.update('0.3.0', releases('v0.3.0'))
        result = self.update('0.2.5', releases('v0.3.0', 'v0.3.1', 'v0.2.5'))
        self.assertEqual(result.downloaded, ['v0.3.1'])
        self.assertIn('Updating the tap to v0.3.1, the latest stable core release, for v0.2.5', result.stdout)
        head = self.remote_head()
        self.assertEqual(self.git(self.remote, 'log', '-1', '--format=%s', head), 'Updated Jet to v0.3.1')
        self.assertEqual(self.changed(head), ['Casks/jet-app.rb', 'Formula/jet.rb'])
        self.assertEqual(self.remote_file('Formula/jet.rb'), rendered('0.3.1')['jet.rb'])
        self.assertEqual(self.remote_file('Casks/jet-app.rb'), rendered('0.3.1')['jet-app.rb'])

        # A rerun of v0.3.1's own job then changes nothing.
        result = self.update('0.3.1', releases('v0.3.0', 'v0.3.1', 'v0.2.5'))
        self.assertIn('Skipped v0.3.1: the tap already has this release', result.stdout)
        self.assertNotIn('Updating the tap', result.stdout)
        self.assertEqual(self.remote_head(), head)

    def test_a_swift_release_after_a_version_bump_leaves_linux_without_a_formula_until_that_tag(self):
        # The release-order rule in docs/core-distribution.md. The Swift
        # release builds with main's core version, so a run after the bump to
        # 0.3.0 replaces the macOS and Linux formula with a macOS-only one.
        self.update('0.2.0', releases('v0.2.0'))
        self.swift('1.0.5', '0.3.0')
        swift_formula = self.remote_file('Formula/jet.rb')
        self.assertIn('\n  depends_on :macos\n', swift_formula)
        self.assertNotIn('linux-gnu', swift_formula)
        self.assertEqual(SWIFT_VERSION_LINE.findall(swift_formula), ['0.3.0'])
        self.assertEqual(self.remote_file('Casks/jet-app.rb'), rendered('0.2.0')['jet-app.rb'])

        # A patch release for the older version cannot close the window: the
        # tap's formula is newer, so it skips both files.
        result = self.update('0.2.1', releases('v0.2.0', 'v0.2.1', 'swift-v1.0.5'))
        self.assertIn("the tap's Formula/jet.rb is newer", result.stdout)
        self.assertEqual(self.remote_file('Formula/jet.rb'), swift_formula)
        self.assertEqual(self.remote_file('Casks/jet-app.rb'), rendered('0.2.0')['jet-app.rb'])

        # The tag at the bumped version closes it.
        self.update('0.3.0', releases('v0.2.0', 'v0.2.1', 'v0.3.0', 'swift-v1.0.5'))
        self.assertEqual(self.remote_file('Formula/jet.rb'), rendered('0.3.0')['jet.rb'])
        self.assertEqual(self.remote_file('Casks/jet-app.rb'), rendered('0.3.0')['jet-app.rb'])

    def test_takes_concurrent_swift_commits_and_never_pushes_a_merged_file(self):
        self.swift('1.0.4', '0.2.0')
        self.update('0.2.0', releases('v0.2.0'))

        # The Swift release pushes its cask after this job checked the tap out.
        self.checkout(self.root / 'core')
        self.swift('1.0.5', '0.2.0')
        swift = self.remote_head()
        self.update('0.3.0', releases('v0.3.0'), fresh=False)
        head = self.remote_head()
        self.assertEqual(self.git(self.remote, 'rev-parse', f'{head}~1'), swift)
        self.assertEqual(self.changed(head), ['Casks/jet-app.rb', 'Formula/jet.rb'])
        self.assertIn('version "1.0.5"', self.remote_file('Casks/jet.rb'))
        self.assertEqual(self.remote_file('Formula/jet.rb'), rendered('0.3.0')['jet.rb'])

        # A concurrent edit that merges cleanly would push a formula nobody
        # published; the job fails instead, and a rerun decides again.
        self.checkout(self.root / 'core')
        maintainer = self.checkout(self.root / 'maintainer')
        with (maintainer / 'Formula/jet.rb').open('a') as file:
            file.write('# Edited by a maintainer\n')
        self.git(maintainer, 'commit', '-am', 'Edited the formula')
        self.git(maintainer, 'push', 'origin', 'main')
        edited = self.remote_head()
        result = self.update('0.4.0', releases('v0.4.0'), fresh=False, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('changed while v0.4.0 was published', result.stderr)
        self.assertEqual(self.remote_head(), edited)
        self.update('0.4.0', releases('v0.4.0'))
        self.assertEqual(self.remote_file('Formula/jet.rb'), rendered('0.4.0')['jet.rb'])

        # A conflicting newer Swift formula fails the push; the rerun skips.
        self.checkout(self.root / 'core')
        self.swift('1.0.6', '0.6.0')
        swift = self.remote_head()
        result = self.update('0.5.0', releases('v0.5.0'), fresh=False, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.remote_head(), swift)
        result = self.update('0.5.0', releases('v0.5.0'))
        self.assertIn("the tap's Formula/jet.rb is newer", result.stdout)
        self.assertEqual(self.remote_head(), swift)


if __name__ == '__main__':
    unittest.main()
