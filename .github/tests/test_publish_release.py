"""Run publish-release.sh against a fake `gh` that keeps the repository's releases.

The Linux app's updater reads releases/latest/download/latest.json and the
jet-app cask's livecheck reads the latest release, so GitHub's Latest must stay
the highest stable core release whatever order tags publish in.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'scripts' / 'publish-release.sh'

# Releases as GitHub keeps them: `make_latest` moves Latest only when asked.
FAKE_GH = '''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
path = os.environ['TEST_STATE']
state = json.load(open(path))
with open(os.environ['TEST_LOG'], 'a') as log:
    log.write(json.dumps(args) + '\\n')
releases = {release['tagName']: release for release in state['releases']}
if args[:2] == ['release', 'view']:
    assert args[3:] == ['--json', 'isDraft'], args
    if args[2] not in releases:
        sys.exit('release not found')
    print(json.dumps({'isDraft': releases[args[2]]['isDraft']}))
elif args[:2] == ['release', 'list']:
    assert args[args.index('--repo') + 1] == 'apexgang/jet', args
    assert args[args.index('--json') + 1] == 'tagName,isDraft,isPrerelease', args
    print(json.dumps(state['releases']))
elif args == ['api', 'repos/apexgang/jet/releases/latest', '--jq', '.tag_name']:
    if not state['latest']:
        sys.exit('Not Found')
    print(state['latest'])
elif args[:2] == ['release', 'create']:
    assert '--draft' in args and '--verify-tag' in args, args
    state['releases'].append({'tagName': args[2], 'isDraft': True, 'isPrerelease': False})
elif args[:2] == ['release', 'upload']:
    assert releases[args[2]]['isDraft'], args
elif args[:2] == ['release', 'edit']:
    release = releases[args[2]]
    assert '--draft=false' in args, args
    release['isDraft'] = False
    release['isPrerelease'] = '--prerelease' in args
    if '--latest' in args:
        state['latest'] = args[2]
    else:
        assert '--latest=false' in args, args
else:
    raise AssertionError(args)
json.dump(state, open(path, 'w'))
'''

# Stands in for release_assets.py, which the release's tests cover.
FAKE_ASSETS = '''import pathlib, sys
args = sys.argv[1:]
dist = pathlib.Path(args[args.index('--dist') + 1])
dist.mkdir(parents=True, exist_ok=True)
for name in ('jet.rb', 'jet-app.rb'):
    (dist / name).write_text('class Jet\\nend\\n')
for name in ('latest.json', 'SHA256SUMS', 'jet-core-0-x86_64-unknown-linux-gnu.tar.gz'):
    (dist / name).write_text('')
with open(pathlib.Path(sys.argv[0]).parent / 'generated', 'a') as log:
    log.write(args[args.index('--tag') + 1] + '\\n')
'''


def release(tag, draft=False, prerelease=False):
    return {'tagName': tag, 'isDraft': draft, 'isPrerelease': prerelease}


class PublishRelease(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.workspace = self.root / 'workspace'
        scripts = self.workspace / '.github/scripts'
        scripts.mkdir(parents=True)
        (scripts / 'release_assets.py').write_text(FAKE_ASSETS)
        self.generated = scripts / 'generated'
        tools = self.root / 'bin'
        tools.mkdir()
        (tools / 'gh').write_text(FAKE_GH)
        (tools / 'gh').chmod(0o755)
        self.runner = self.root / 'runner'
        (self.runner / 'jet-desktop').mkdir(parents=True)
        (self.runner / 'jet-desktop' / 'jet_0_amd64.deb').write_text('')
        self.state = self.root / 'state.json'
        self.log = self.root / 'gh.log'
        self.output = self.root / 'output'
        self.environment = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull,
                                PATH=f'{tools}{os.pathsep}{os.environ["PATH"]}', GITHUB_REPOSITORY='apexgang/jet',
                                RUNNER_TEMP=str(self.runner), GITHUB_OUTPUT=str(self.output),
                                TEST_STATE=str(self.state), TEST_LOG=str(self.log))
        git = ['git', '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid']
        subprocess.run([*git, 'init', '--initial-branch=main'], cwd=self.workspace, check=True, capture_output=True)
        subprocess.run([*git, 'commit', '--allow-empty', '-m', 'Tagged'], cwd=self.workspace, check=True,
                       capture_output=True)
        self.environment['GITHUB_SHA'] = subprocess.check_output(
            ['git', 'rev-parse', 'HEAD'], cwd=self.workspace, text=True).strip()

    def publish(self, tag, releases, latest, *args):
        self.state.write_text(json.dumps({'releases': releases, 'latest': latest}))
        self.log.write_text('')
        result = subprocess.run(['bash', str(SCRIPT), *args], cwd=self.workspace, capture_output=True, text=True,
                                env=dict(self.environment, GITHUB_REF_NAME=tag))
        result.state = json.loads(self.state.read_text())
        result.calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        result.writes = [call[:2] for call in result.calls if call[:2] in (
            ['release', 'create'], ['release', 'upload'], ['release', 'edit'])]
        result.generated = self.generated.read_text().split() if self.generated.exists() else []
        return result

    def edit(self, result):
        [edit] = [call for call in result.calls if call[:2] == ['release', 'edit']]
        return edit[3:]

    def test_the_highest_stable_release_takes_latest(self):
        result = self.publish('v0.10.0', [release('v0.9.0'), release('swift-v1.0.4')], 'v0.9.0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.generated, ['v0.10.0'])
        self.assertEqual(result.writes, [['release', 'create'], ['release', 'upload'], ['release', 'edit']])
        self.assertEqual(self.edit(result), ['--draft=false', '--latest'])
        self.assertEqual(result.state['latest'], 'v0.10.0')

        # The first core release takes Latest from the Swift app's release.
        result = self.publish('v0.2.0', [release('swift-v1.0.4')], 'swift-v1.0.4')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.state['latest'], 'v0.2.0')

    def test_a_patch_for_an_older_version_never_takes_latest(self):
        # v0.2.5 is tagged after v0.3.0 and v0.10.0. Drafts and prereleases of
        # newer versions do not count.
        releases = [release('v0.2.0'), release('v0.3.0'), release('v0.10.0'), release('swift-v1.0.9'),
                    release('v0.11.0', draft=True), release('v0.12.0-rc.1', prerelease=True)]
        result = self.publish('v0.2.5', releases, 'v0.10.0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.edit(result), ['--draft=false', '--latest=false'])
        self.assertEqual(result.state['latest'], 'v0.10.0')
        self.assertIn(release('v0.2.5'), result.state['releases'])

        # Only drafts and prereleases are newer: the patch takes Latest.
        result = self.publish('v0.2.5', [release('v0.2.0'), release('v0.3.0', draft=True),
                                         release('v0.4.0-rc.1', prerelease=True)], 'v0.2.0')
        self.assertEqual(self.edit(result), ['--draft=false', '--latest'])
        self.assertEqual(result.state['latest'], 'v0.2.5')

    def test_a_prerelease_never_takes_latest(self):
        result = self.publish('v0.4.0-rc.1', [release('v0.3.0')], 'v0.3.0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.edit(result), ['--draft=false', '--prerelease', '--latest=false'])
        self.assertEqual(result.state['latest'], 'v0.3.0')

    def test_a_draft_left_by_an_earlier_attempt_is_completed(self):
        result = self.publish('v0.3.0', [release('v0.2.0'), release('v0.3.0', draft=True)], 'v0.2.0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.writes, [['release', 'upload'], ['release', 'edit']])
        self.assertEqual(result.state['latest'], 'v0.3.0')

    def test_a_published_release_only_checks_latest(self):
        # The artifacts may have expired: nothing is downloaded or generated.
        releases = [release('v0.2.0'), release('v0.2.5'), release('v0.3.0'), release('swift-v1.0.4')]
        for tag in ('v0.2.5', 'v0.3.0'):
            with self.subTest(tag=tag):
                result = self.publish(tag, releases, 'v0.3.0')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.generated, [])
                self.assertEqual(result.writes, [])

        # Another release holding Latest fails the run, whichever tag reruns.
        for latest in ('swift-v1.0.4', 'v0.2.5'):
            with self.subTest(latest=latest):
                result = self.publish('v0.2.5', releases, latest)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(f"GitHub's latest release is {latest}, not the highest stable core release v0.3.0",
                              result.stderr)
                self.assertIn('gh release edit v0.3.0 --latest', result.stderr)
                self.assertEqual(result.writes, [])

        # A published prerelease leaves Latest alone.
        result = self.publish('v0.4.0-rc.1', [*releases, release('v0.4.0-rc.1', prerelease=True)], 'swift-v1.0.4')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.writes, [])

    def test_check_reports_whether_the_release_is_published(self):
        for releases, published in (([], 'false'), ([release('v0.3.0', draft=True)], 'false'),
                                    ([release('v0.3.0')], 'true')):
            with self.subTest(releases=releases):
                self.output.write_text('')
                result = self.publish('v0.3.0', releases, None, '--check')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.output.read_text(), f'published={published}\n')
                self.assertEqual([call[:2] for call in result.calls], [['release', 'view']])
                self.assertEqual(result.generated, [])


class PublishJob(unittest.TestCase):
    def test_a_published_release_downloads_no_artifacts(self):
        workflow = (Path(__file__).resolve().parents[1] / 'workflows' / 'release.yml').read_text()
        job = workflow.split('\n  publish:\n', 1)[1].split('\n  homebrew:\n', 1)[0]
        steps = job.split('\n      - ')[1:]
        self.assertTrue(steps[0].startswith('uses: actions/checkout@'))
        self.assertTrue(steps[1].startswith('name: Check whether the release is published\n        id: release\n'))
        self.assertTrue(steps[1].endswith('\n        run: bash .github/scripts/publish-release.sh --check'))
        downloads = [step for step in steps if step.startswith('uses: actions/download-artifact@')]
        self.assertEqual(len(downloads), 2)
        for step in downloads:
            self.assertIn("\n        if: ${{ steps.release.outputs.published != 'true' }}\n", step)
        self.assertTrue(steps[-1].startswith('name: Publish complete release\n'))
        self.assertNotIn('if:', steps[-1])
        self.assertTrue(steps[-1].endswith('        run: bash .github/scripts/publish-release.sh\n'))


if __name__ == '__main__':
    unittest.main()
